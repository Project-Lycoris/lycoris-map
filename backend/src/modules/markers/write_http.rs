//! 阶段 3 点位写入/收藏/审核 HTTP 层：薄 handler、认证提取器与响应本地化。
//!
//! 本模块不含业务事务与 SQL：身份由 [`CurrentUser`]/[`AdminUser`] 提取后，
//! 只从 `Identity.user` 当前数据库行构造 [`Actor`]（绝不从请求体填 owner/admin），再调用
//! 已验收的 [`MarkerWriteService`]。成功 `MarkerRow`/`Vec<MarkerRow>` 一律经
//! [`MarkerService::localize`] 生成与读取接口同形的 26 字段响应并带语言 `Vary`，
//! 不直接序列化数据库行。
//!
//! User endpoints require `CurrentUser`; every admin endpoint requires the
//! current database role ADMIN through `AdminUser`. No secondary password.

use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::app::AppState;
use crate::auth::{AdminUser, CurrentUser, Identity};
use crate::error::{ApiError, ErrorShape};
use crate::modules::markers::http::{json_marker, json_markers};
use crate::modules::markers::localization;
use crate::modules::markers::model::{MarkerRow, Viewer};
use crate::modules::markers::moderation::proposer_stats;
use crate::modules::markers::write::MarkerSearchQuery;
use crate::modules::markers::write_model::{
    Actor, EditProposalDto, MarkerCreateRequest, MarkerUpdateRequest, WriteError,
};
use crate::web::{self, MarkerJsonBody};

/// 构建点位写入/收藏/审核路由。静态段优先于 `/{id}`，不会互相遮蔽。
pub fn router() -> Router<AppState> {
    Router::new()
        // 用户写与本人读取（登录）。
        .route("/api/markers", post(create))
        .route(
            "/api/markers/{id}",
            patch(update_marker).delete(delete_marker),
        )
        .route(
            "/api/markers/{id}/favorite",
            post(add_favorite).delete(remove_favorite),
        )
        .route("/api/markers/me/favorites", get(my_favorites))
        .route("/api/markers/me/created", get(my_created))
        .route("/api/markers/me/submissions", get(my_submissions))
        .route(
            "/api/markers/me/favorites/details",
            get(my_favorite_details),
        )
        // 管理员诊断读：只要求当前数据库角色为 ADMIN，不要求二次验证。
        .route("/api/markers/all", get(list_all))
        // Current database role ADMIN.
        .route("/api/admin/markers/pending", get(admin_pending))
        .route("/api/admin/markers/pending-edits", get(admin_pending_edits))
        .route("/api/admin/markers/all", get(admin_all))
        .route("/api/admin/markers/search", get(admin_search))
        .route("/api/admin/moderation/batch", post(admin_batch))
        .route("/api/admin/moderation/events", get(admin_events))
        .route("/api/admin/markers/{id}/approve", post(admin_approve))
        .route("/api/admin/markers/{id}/reject", post(admin_reject))
        .route("/api/admin/markers/{id}/restore", post(admin_restore))
        .route(
            "/api/admin/markers/{id}",
            patch(admin_update).delete(admin_delete),
        )
        .route(
            "/api/admin/markers/edit-proposals/{id}/approve",
            post(admin_approve_proposal),
        )
        .route(
            "/api/admin/markers/edit-proposals/{id}/reject",
            post(admin_reject_proposal),
        )
}

#[derive(Debug, Deserialize)]
struct LangQuery {
    lang: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveBody {
    changes: Option<MarkerUpdateRequest>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectBody {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams {
    page: Option<i64>,
    size: Option<i64>,
    q: Option<String>,
    status: Option<String>,
    category: Option<String>,
    deactivated: Option<bool>,
    sort: Option<String>,
    lang: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsParams {
    page: Option<i64>,
    size: Option<i64>,
    marker_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchRequest {
    items: Vec<BatchItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchItem {
    kind: String,
    id: i64,
    decision: String,
    reason: Option<String>,
}

/// 可选 JSON 请求体：无 body、空白 body 都等价于旧客户端（`None`）；
/// 非空 body 不要求 Content-Type，但必须是合法 JSON，否则 400。
pub struct OptionalMarkerJsonBody<T>(pub Option<T>);

impl<S, T> FromRequest<S> for OptionalMarkerJsonBody<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes =
            match axum::body::to_bytes(req.into_body(), crate::config::REQUEST_BODY_LIMIT_BYTES)
                .await
            {
                Ok(bytes) => bytes,
                Err(error) if crate::web::is_length_limit_error(&error) => {
                    return Err(crate::multipart::payload_too_large_response());
                }
                Err(_) => return Err(web::text(StatusCode::BAD_REQUEST, "请求体读取失败")),
            };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(None));
        }
        serde_json::from_slice(&bytes)
            .map(|value| Self(Some(value)))
            .map_err(|_| web::text(StatusCode::BAD_REQUEST, "请求参数不合法"))
    }
}

/// 只从当前数据库账号构造身份快照，`public_id` 为归属依据、`username` 仅作快照。
fn actor_of(identity: &Identity) -> Actor {
    Actor::new(
        identity.user.public_id.to_string(),
        identity.user.username_or_empty().to_string(),
        identity.is_admin(),
    )
}

fn response_language(query: &LangQuery, headers: &HeaderMap) -> &'static str {
    localization::for_read(query.lang.as_deref(), headers)
}

fn write_error(error: WriteError) -> Response {
    error.into_reply(ErrorShape::Text).into_response()
}

fn api_text(error: ApiError) -> Response {
    error.into_reply(ErrorShape::Text).into_response()
}

fn empty_ok() -> Response {
    web::empty(StatusCode::OK)
}

/// 把单行本地化为 26 字段响应（带语言 `Vary`）。
async fn localize_one(state: &AppState, row: MarkerRow, lang: &'static str) -> Response {
    json_marker(&state.markers.committed_row(row, lang).await)
}

/// 把多行本地化为 26 字段响应数组（带语言 `Vary`）。
async fn localize_many(state: &AppState, rows: Vec<MarkerRow>, lang: &'static str) -> Response {
    match state.markers.localize(rows, lang).await {
        Ok(dtos) => json_markers(&dtos),
        Err(error) => api_text(error),
    }
}

/// POST /api/markers
async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerCreateRequest>,
) -> Response {
    let actor = actor_of(&user.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .create_marker(&actor, request_language, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// PATCH /api/markers/{id}：普通用户只创建待审编辑提案并返回未修改的原点位。
async fn update_marker(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerUpdateRequest>,
) -> Response {
    let actor = actor_of(&user.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .create_edit_proposal(&actor, request_language, id, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// DELETE /api/markers/{id}
async fn delete_marker(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .delete_owned_marker(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/markers/{id}/favorite
async fn add_favorite(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .add_favorite(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// DELETE /api/markers/{id}/favorite
async fn remove_favorite(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .remove_favorite(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/favorites：可见收藏点位 ID 列表。
async fn my_favorites(State(state): State<AppState>, user: CurrentUser) -> Response {
    match state.markers_write.favorite_ids(&actor_of(&user.0)).await {
        Ok(ids) => Json(ids).into_response(),
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/created
async fn my_created(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state.markers_write.list_created(&actor_of(&user.0)).await {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/favorites/details
async fn my_favorite_details(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .favorite_markers(&actor_of(&user.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/all：管理员全部点位读取，不要求二次验证。
async fn list_all(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .list_all_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/pending
async fn admin_pending(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .pending_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let public_ids: Vec<String> = rows
                .iter()
                .filter_map(|row| row.user_public_id.clone())
                .collect();
            let stats = match proposer_stats(&state.db, &public_ids).await {
                Ok(stats) => stats,
                Err(_) => return write_error(WriteError::Unavailable),
            };
            let lang = response_language(&query, &headers);
            let dtos = match state.markers.localize(rows, lang).await {
                Ok(dtos) => dtos,
                Err(error) => return api_text(error),
            };
            let mut items = Vec::with_capacity(dtos.len());
            for dto in dtos {
                let stats = dto
                    .user_public_id
                    .as_deref()
                    .and_then(|public_id| stats.get(public_id))
                    .copied()
                    .unwrap_or_default();
                let mut value = web::to_json(&dto);
                if let Some(object) = value.as_object_mut() {
                    object.insert("proposerSubmissionStats".to_string(), web::to_json(&stats));
                }
                items.push(value);
            }
            let response = Json(items).into_response();
            let mut response = response;
            crate::modules::markers::http::with_vary(&mut response);
            response
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/pending-edits：普通 JSON，提案 DTO（含提交者统计）。
async fn admin_pending_edits(State(state): State<AppState>, admin: AdminUser) -> Response {
    match state
        .markers_write
        .pending_edit_proposals(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let public_ids: Vec<String> = rows
                .iter()
                .filter_map(|row| row.proposer_public_id.clone())
                .collect();
            let stats = match proposer_stats(&state.db, &public_ids).await {
                Ok(stats) => stats,
                Err(_) => return write_error(WriteError::Unavailable),
            };
            let items: Vec<EditProposalDto> = rows
                .into_iter()
                .map(|row| {
                    let public_id = row.proposer_public_id.clone();
                    let mut dto = EditProposalDto::from(row);
                    dto.proposer_submission_stats = public_id
                        .as_deref()
                        .and_then(|public_id| stats.get(public_id))
                        .copied()
                        .unwrap_or_default();
                    dto
                })
                .collect();
            Json(items).into_response()
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/all
async fn admin_all(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .list_all_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/search：服务端分页搜索全部点位。
async fn admin_search(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Response {
    let page = params.page.unwrap_or(0).max(0);
    let size = params.size.unwrap_or(20).clamp(1, 100);
    let query = MarkerSearchQuery {
        q: params.q.unwrap_or_default(),
        status: params.status.filter(|value| !value.trim().is_empty()),
        category: params.category.filter(|value| !value.trim().is_empty()),
        deactivated: params.deactivated,
        sort: params
            .sort
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "updated_desc".to_string()),
        page,
        size,
    };
    let (total, rows) = match state
        .markers_write
        .search_markers(&actor_of(&admin.0), &query)
        .await
    {
        Ok(result) => result,
        Err(error) => return write_error(error),
    };
    let total_pages = if total == 0 {
        0
    } else {
        (total + size - 1) / size
    };
    let lang = localization::for_read(params.lang.as_deref(), &headers);
    let notes: Vec<Option<String>> = rows.iter().map(|row| row.review_note.clone()).collect();
    let markers: Vec<MarkerRow> = rows.into_iter().map(|row| row.marker).collect();
    let dtos = match state.markers.localize(markers, lang).await {
        Ok(dtos) => dtos,
        Err(error) => return api_text(error),
    };
    let items: Vec<serde_json::Value> = dtos
        .into_iter()
        .zip(notes)
        .map(|(dto, review_note)| {
            let mut value = web::to_json(&dto);
            if let Some(object) = value.as_object_mut() {
                object.insert("reviewNote".to_string(), json!(review_note));
            }
            value
        })
        .collect();
    let mut response = web::json(
        StatusCode::OK,
        json!({
            "page": page,
            "size": size,
            "totalPages": total_pages,
            "totalElements": total,
            "items": items,
        }),
    );
    crate::modules::markers::http::with_vary(&mut response);
    response
}

/// POST /api/admin/moderation/batch：逐条独立事务，允许部分成功。
async fn admin_batch(
    State(state): State<AppState>,
    admin: AdminUser,
    body: MarkerJsonBody<BatchRequest>,
) -> Response {
    let request = body.0;
    if request.items.is_empty() || request.items.len() > 50 {
        return web::text(StatusCode::BAD_REQUEST, "批量条数必须为 1–50");
    }
    let mut seen = std::collections::HashSet::new();
    for item in &request.items {
        if !matches!(item.kind.as_str(), "marker" | "edit" | "image") {
            return web::text(StatusCode::BAD_REQUEST, "kind 不合法");
        }
        if !matches!(item.decision.as_str(), "approve" | "reject") {
            return web::text(StatusCode::BAD_REQUEST, "decision 不合法");
        }
        if !seen.insert((item.kind.clone(), item.id)) {
            return web::text(StatusCode::BAD_REQUEST, "批量项重复");
        }
    }

    let actor = actor_of(&admin.0);
    let reviewer = admin.0.user.username_or_empty().to_string();
    let public_id = admin.0.user.public_id.to_string();
    let viewer = Viewer {
        public_id: Some(public_id.as_str()),
        role: admin.0.user.role.as_str(),
        deleted: admin.0.user.deleted,
    };
    let mut results = Vec::with_capacity(request.items.len());
    for item in request.items {
        let outcome: Result<(), (StatusCode, String)> =
            match (item.kind.as_str(), item.decision.as_str()) {
                ("marker", "approve") => state
                    .markers_write
                    .approve_marker(&actor, item.id)
                    .await
                    .map(|_| ())
                    .map_err(write_failure),
                ("marker", "reject") => state
                    .markers_write
                    .reject_marker_with_reason(&actor, item.id, item.reason.as_deref())
                    .await
                    .map(|_| ())
                    .map_err(write_failure),
                ("edit", "approve") => state
                    .markers_write
                    .approve_edit_proposal(&actor, item.id)
                    .await
                    .map(|_| ())
                    .map_err(write_failure),
                ("edit", "reject") => state
                    .markers_write
                    .reject_edit_proposal_with_reason(&actor, item.id, item.reason.as_deref())
                    .await
                    .map_err(write_failure),
                ("image", "approve") => state
                    .media
                    .approve_image_proposal(item.id, &viewer, &reviewer)
                    .await
                    .map(|_| ())
                    .map_err(media_failure),
                ("image", "reject") => state
                    .media
                    .reject_image_proposal_with_reason(
                        item.id,
                        &viewer,
                        &reviewer,
                        item.reason.as_deref(),
                    )
                    .await
                    .map_err(media_failure),
                _ => unreachable!("kind/decision 已校验"),
            };
        let mut result = json!({
            "kind": item.kind,
            "id": item.id,
            "ok": outcome.is_ok(),
        });
        if let Err((status, message)) = outcome
            && let Some(object) = result.as_object_mut()
        {
            object.insert("status".to_string(), json!(status.as_u16()));
            object.insert("message".to_string(), json!(message));
        }
        results.push(result);
    }
    web::json(StatusCode::OK, json!({ "results": results }))
}

/// GET /api/admin/moderation/events：审核记录分页。
async fn admin_events(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<EventsParams>,
) -> Response {
    let page = params.page.unwrap_or(0).max(0);
    let size = params.size.unwrap_or(50).clamp(1, 100);
    match state
        .markers_write
        .moderation_events(&actor_of(&admin.0), params.marker_id, page, size)
        .await
    {
        Ok((total, items)) => {
            let total_pages = if total == 0 {
                0
            } else {
                (total + size - 1) / size
            };
            web::json(
                StatusCode::OK,
                json!({
                    "page": page,
                    "size": size,
                    "totalPages": total_pages,
                    "totalElements": total,
                    "items": items,
                }),
            )
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/submissions：当前用户自己的三类提交。
async fn my_submissions(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(_query): Query<LangQuery>,
) -> Response {
    match state.markers_write.my_submissions(&actor_of(&user.0)).await {
        Ok(items) => Json(items).into_response(),
        Err(error) => write_error(error),
    }
}

fn write_failure(error: WriteError) -> (StatusCode, String) {
    (error.status(), error.to_string())
}

fn media_failure(error: crate::media::MediaServiceError) -> (StatusCode, String) {
    (error.status(), error.message())
}

/// POST /api/admin/markers/{id}/approve
#[allow(clippy::too_many_arguments)]
async fn admin_approve(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    body: OptionalMarkerJsonBody<ApproveBody>,
) -> Response {
    let actor = actor_of(&admin.0);
    let request_language = localization::from_headers(&headers);
    let changes = body.0.and_then(|body| body.changes);
    match state
        .markers_write
        .approve_marker_with(&actor, id, changes, request_language)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/{id}/reject
async fn admin_reject(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    body: OptionalMarkerJsonBody<RejectBody>,
) -> Response {
    let reason = body.0.and_then(|body| body.reason);
    match state
        .markers_write
        .reject_marker_with_reason(&actor_of(&admin.0), id, reason.as_deref())
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// PATCH /api/admin/markers/{id}：管理员直接编辑，不生成提案。
async fn admin_update(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerUpdateRequest>,
) -> Response {
    let actor = actor_of(&admin.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .admin_update_marker(&actor, request_language, id, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// DELETE /api/admin/markers/{id}
async fn admin_delete(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .admin_delete_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/{id}/restore: restore without approving or changing visibility.
async fn admin_restore(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .admin_restore_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/edit-proposals/{id}/approve
#[allow(clippy::too_many_arguments)]
async fn admin_approve_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    body: OptionalMarkerJsonBody<ApproveBody>,
) -> Response {
    let actor = actor_of(&admin.0);
    let request_language = localization::from_headers(&headers);
    let changes = body.0.and_then(|body| body.changes);
    match state
        .markers_write
        .approve_edit_proposal_with_changes(&actor, id, changes, request_language)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/edit-proposals/{id}/reject
async fn admin_reject_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    body: OptionalMarkerJsonBody<RejectBody>,
) -> Response {
    let reason = body.0.and_then(|body| body.reason);
    match state
        .markers_write
        .reject_edit_proposal_with_reason(&actor_of(&admin.0), id, reason.as_deref())
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}
