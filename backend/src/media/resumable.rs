//! Small-photo resume protocol. PostgreSQL commits each offset with its bytes;
//! completion commits exactly one proposal together with its durable receipt.
use crate::{
    app::AppState,
    auth::CurrentUser,
    media::{MAX_UPLOAD_BYTES, MediaDirectory, MediaError, MediaRepository},
    modules::markers::model::{Viewer, can_view},
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

const CHUNK: usize = 256 * 1024;
// Bound work that must finish even if the HTTP waiter is cancelled.
static COMPLETIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Debug)]
struct UploadError(StatusCode, &'static str);
impl IntoResponse for UploadError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}
impl From<sqlx::Error> for UploadError {
    fn from(error: sqlx::Error) -> Self {
        tracing::warn!(code = ?error.as_database_error().and_then(|e| e.code()), "photo resume database operation failed");
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "Upload service temporarily unavailable",
        )
    }
}
impl From<MediaError> for UploadError {
    fn from(error: MediaError) -> Self {
        match error {
            MediaError::Busy => Self(StatusCode::SERVICE_UNAVAILABLE, "Image processing busy"),
            MediaError::Empty
            | MediaError::Dimensions
            | MediaError::UnsupportedFormat
            | MediaError::Decode => Self(
                StatusCode::BAD_REQUEST,
                "Invalid image: use JPEG, PNG, GIF or WebP within the image limits",
            ),
            MediaError::TooLarge => Self(StatusCode::PAYLOAD_TOO_LARGE, "Image exceeds 5 MiB"),
            _ => Self(StatusCode::INTERNAL_SERVER_ERROR, "Could not save image"),
        }
    }
}
type Result<T> = std::result::Result<T, UploadError>;

#[derive(sqlx::FromRow)]
struct Upload {
    upload_id: Uuid,
    marker_id: i64,
    total_bytes: i32,
    sha256: String,
    received_bytes: i32,
    status: String,
    expires_at: DateTime<Utc>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    upload_id: String,
    marker_id: i64,
    total_bytes: i32,
    received_bytes: i32,
    chunk_size: usize,
    status: String,
}
impl Upload {
    fn receipt(&self) -> Json<Receipt> {
        Json(Receipt {
            upload_id: self.upload_id.to_string(),
            marker_id: self.marker_id,
            total_bytes: self.total_bytes,
            received_bytes: self.received_bytes,
            chunk_size: CHUNK,
            status: self.status.clone(),
        })
    }
    fn active(&self) -> Result<()> {
        if self.status == "EXPIRED" || (self.status != "COMPLETED" && self.expires_at <= Utc::now())
        {
            return Err(UploadError(
                StatusCode::GONE,
                "Upload expired; select the photo again",
            ));
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Start {
    client_request_id: String,
    total_bytes: i32,
    sha256: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/markers/{id}/image-uploads", post(start))
        .route("/api/markers/{id}/image-uploads/{upload}", get(status))
        .route(
            "/api/markers/{id}/image-uploads/{upload}/chunks/{offset}",
            post(chunk),
        )
        .route(
            "/api/markers/{id}/image-uploads/{upload}/complete",
            post(complete),
        )
        .layer(axum::middleware::map_response(no_store))
}
async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
fn parse_upload_id(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|_| UploadError(StatusCode::BAD_REQUEST, "Invalid upload ID"))
}
fn identity(user: &CurrentUser) -> Uuid {
    user.0.user.public_id
}
fn viewer<'a>(user: &'a CurrentUser, public_id: &'a str) -> Viewer<'a> {
    Viewer {
        public_id: Some(public_id),
        role: user.0.user.role.as_str(),
        deleted: user.0.user.deleted,
    }
}
async fn visible(state: &AppState, user: &CurrentUser, id: i64) -> Result<()> {
    let repo = MediaRepository::new(state.db.clone());
    let marker = repo
        .marker_by_id(id)
        .await?
        .ok_or(UploadError(StatusCode::NOT_FOUND, "Place not found"))?;
    if !can_view(&marker, Some(&viewer(user, &identity(user).to_string()))) {
        return Err(UploadError(StatusCode::NOT_FOUND, "Place not found"));
    }
    Ok(())
}
async fn locked(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: i64,
    upload: Uuid,
) -> Result<Upload> {
    // Literal SQL with bound request values; large staged bytes are read only at completion.
    let row = sqlx::query_as::<_, Upload>("SELECT upload_id, marker_id, total_bytes, sha256, received_bytes, status, expires_at FROM marker_image_uploads WHERE upload_id=$1 AND owner_public_id=$2 AND marker_id=$3 FOR UPDATE")
        .bind(upload).bind(identity(user)).bind(id).fetch_optional(conn).await?
        .ok_or(UploadError(StatusCode::NOT_FOUND, "Upload not found"))?;
    row.active()?;
    Ok(row)
}
async fn start(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(input): Json<Start>,
) -> Result<Json<Receipt>> {
    visible(&state, &user, id).await?;
    let request_id = Uuid::parse_str(&input.client_request_id)
        .map_err(|_| UploadError(StatusCode::BAD_REQUEST, "Invalid upload request ID"))?;
    if input.total_bytes < 1
        || input.total_bytes as usize > MAX_UPLOAD_BYTES
        || input.sha256.len() != 64
        || !input
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(UploadError(
            StatusCode::BAD_REQUEST,
            "Invalid image size or SHA256",
        ));
    }
    if user.0.user.username_or_empty().chars().count() > crate::users::USERNAME_MAX {
        return Err(UploadError(
            StatusCode::BAD_REQUEST,
            "Username too long for image contributions",
        ));
    }
    // Bounded opportunistic expiry; pending bytes are reclaimed, receipts remain.
    sqlx::query("UPDATE marker_image_uploads SET status='EXPIRED', staged_bytes=''::bytea WHERE upload_id IN (SELECT upload_id FROM marker_image_uploads WHERE status='UPLOADING' AND expires_at <= now() ORDER BY expires_at LIMIT 32 FOR UPDATE SKIP LOCKED)")
        .execute(&state.db).await?;
    let mut tx = state.db.begin().await?;
    // Serialize per-account quota admission, including concurrent new UUIDs.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 5345))")
        .bind(identity(&user).to_string())
        .execute(&mut *tx)
        .await?;
    let old = sqlx::query_as::<_, Upload>("SELECT upload_id, marker_id, total_bytes, sha256, received_bytes, status, expires_at FROM marker_image_uploads WHERE owner_public_id=$1 AND client_request_id=$2 FOR UPDATE")
        .bind(identity(&user)).bind(request_id).fetch_optional(&mut *tx).await?;
    if let Some(old) = old {
        if old.marker_id != id || old.total_bytes != input.total_bytes || old.sha256 != input.sha256
        {
            return Err(UploadError(
                StatusCode::CONFLICT,
                "Upload ID belongs to different content",
            ));
        }
        tx.commit().await?;
        old.active()?;
        return Ok(old.receipt());
    }
    let (count, bytes): (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(total_bytes),0)::bigint FROM marker_image_uploads WHERE owner_public_id=$1 AND status='UPLOADING' AND expires_at > now()")
        .bind(identity(&user)).fetch_one(&mut *tx).await?;
    if count >= 4 || bytes + i64::from(input.total_bytes) > 20 * 1024 * 1024 {
        return Err(UploadError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many unfinished photo uploads",
        ));
    }
    let row = sqlx::query_as::<_, Upload>("INSERT INTO marker_image_uploads (upload_id, owner_public_id, marker_id, client_request_id, total_bytes, sha256) VALUES ($1,$2,$3,$4,$5,$6) RETURNING upload_id, marker_id, total_bytes, sha256, received_bytes, status, expires_at")
        .bind(Uuid::new_v4()).bind(identity(&user)).bind(id).bind(request_id).bind(input.total_bytes).bind(input.sha256)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(row.receipt())
}
async fn status(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, upload)): Path<(i64, String)>,
) -> Result<Json<Receipt>> {
    visible(&state, &user, id).await?;
    let mut tx = state.db.begin().await?;
    let row = locked(&mut tx, &user, id, parse_upload_id(&upload)?).await?;
    tx.commit().await?;
    Ok(row.receipt())
}
async fn chunk(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, upload, offset)): Path<(i64, String, i32)>,
    req: Request,
) -> Result<Json<Receipt>> {
    visible(&state, &user, id).await?;
    if req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(UploadError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Use application/octet-stream",
        ));
    }
    let bytes = to_bytes(req.into_body(), CHUNK).await.map_err(|error| {
        if crate::web::is_length_limit_error(&error) {
            UploadError(StatusCode::PAYLOAD_TOO_LARGE, "Chunk exceeds 256 KiB")
        } else {
            UploadError(
                StatusCode::REQUEST_TIMEOUT,
                "Chunk transfer interrupted; resume from confirmed offset",
            )
        }
    })?;
    let mut tx = state.db.begin().await?;
    let upload = parse_upload_id(&upload)?;
    let mut row = locked(&mut tx, &user, id, upload).await?;
    if row.status == "COMPLETED" {
        return Ok(row.receipt());
    }
    if offset < 0
        || !(offset as usize).is_multiple_of(CHUNK)
        || bytes.is_empty()
        || offset > row.received_bytes
        || offset as i64 + bytes.len() as i64 > i64::from(row.total_bytes)
        || bytes.len() != CHUNK.min((row.total_bytes - offset).max(0) as usize)
    {
        return Err(UploadError(
            StatusCode::CONFLICT,
            "Chunk offset or length does not match upload",
        ));
    }
    if offset < row.received_bytes {
        let same: bool = sqlx::query_scalar("SELECT substring(staged_bytes FROM $2 + 1 FOR $3) = $4 FROM marker_image_uploads WHERE upload_id=$1")
            .bind(upload).bind(offset).bind(bytes.len() as i32).bind(bytes.as_ref()).fetch_one(&mut *tx).await?;
        if !same {
            return Err(UploadError(
                StatusCode::CONFLICT,
                "Chunk content differs from confirmed bytes",
            ));
        }
    } else {
        sqlx::query("UPDATE marker_image_uploads SET staged_bytes=staged_bytes || $2::bytea, received_bytes=received_bytes+$3 WHERE upload_id=$1")
            .bind(upload).bind(bytes.as_ref()).bind(bytes.len() as i32).execute(&mut *tx).await?;
        row.received_bytes += bytes.len() as i32;
    }
    tx.commit().await?;
    Ok(row.receipt())
}
async fn complete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, upload)): Path<(i64, String)>,
) -> Result<Json<Receipt>> {
    let permit = COMPLETIONS
        .try_acquire()
        .map_err(|_| UploadError(StatusCode::SERVICE_UNAVAILABLE, "Image completion busy"))?;
    // Dropping the HTTP waiter must not abandon a file written by spawn_blocking.
    // This bounded worker finishes its transaction or known-failure cleanup.
    tokio::spawn(async move {
        let _permit = permit;
        complete_upload(state, user, id, upload).await
    })
    .await
    .map_err(|_| {
        UploadError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Image completion interrupted",
        )
    })?
}
async fn complete_upload(
    state: AppState,
    user: CurrentUser,
    id: i64,
    upload: String,
) -> Result<Json<Receipt>> {
    visible(&state, &user, id).await?;
    let mut tx: Transaction<'_, Postgres> = state.db.begin().await?;
    let upload = parse_upload_id(&upload)?;
    let mut row = locked(&mut tx, &user, id, upload).await?;
    if row.status == "COMPLETED" {
        return Ok(row.receipt());
    }
    if row.received_bytes != row.total_bytes {
        return Err(UploadError(
            StatusCode::CONFLICT,
            "Upload has missing bytes",
        ));
    }
    let bytes: Vec<u8> =
        sqlx::query_scalar("SELECT staged_bytes FROM marker_image_uploads WHERE upload_id=$1")
            .bind(upload)
            .fetch_one(&mut *tx)
            .await?;
    if Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        != row.sha256
    {
        return Err(UploadError(
            StatusCode::BAD_REQUEST,
            "Image checksum does not match",
        ));
    }
    // Keep the upload lock through decoding. ImageStore bounds concurrent decoders.
    // Never delete a stored file after an ambiguous commit; it may be referenced.
    let stored = state
        .images
        .save(
            MediaDirectory::Markers,
            &format!("proposal-marker-{id}"),
            bytes,
        )
        .await?;
    let prepared: Result<()> = async {
    let repo = MediaRepository::new(state.db.clone());
    let marker = repo
        .lock_marker_by_id(&mut tx, id)
        .await?
        .ok_or(UploadError(StatusCode::NOT_FOUND, "Place not found"))?;
    let owner = identity(&user).to_string();
    if !can_view(&marker, Some(&viewer(&user, &owner))) {
        return Err(UploadError(StatusCode::NOT_FOUND, "Place not found"));
    }
    let proposal_id = repo
        .insert_image_proposal(
            &mut tx,
            id,
            &marker.title,
            user.0.user.username_or_empty(),
            Some(&owner),
            &stored.url,
        )
        .await?;
    sqlx::query("UPDATE marker_image_uploads SET status='COMPLETED', proposal_id=$2, staged_bytes=''::bytea WHERE upload_id=$1")
        .bind(upload).bind(proposal_id).execute(&mut *tx).await?;
        Ok(())
    }.await;
    if let Err(error) = prepared {
        // COMMIT has not been attempted: this new file cannot be referenced by a committed proposal.
        let _ = tx.rollback().await;
        if state.images.remove_new(&stored).await.is_err() {
            tracing::warn!("could not remove uncommitted resume image");
        }
        return Err(error);
    }
    tx.commit().await?;
    row.status = "COMPLETED".to_owned();
    Ok(row.receipt())
}
