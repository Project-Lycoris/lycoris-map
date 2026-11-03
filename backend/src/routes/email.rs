//! Anonymous verification and recovery. Send responses never disclose account existence.
use crate::{
    app::AppState,
    email_verification::{CodeError, Purpose, normalize_email},
    error::ApiResponse,
    password,
    ratelimit::RateLimitOutcome,
    users,
    web::{self, JsonBody},
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;

#[derive(Deserialize)]
pub struct SendCodeRequest {
    email: String,
    purpose: Purpose,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    email: String,
    verification_code: String,
    new_password: String,
}

pub fn code_error(error: CodeError) -> Response {
    let (status, code, message, retry) = match error {
        CodeError::Invalid => (
            StatusCode::BAD_REQUEST,
            40021,
            "Verification code is invalid or expired.",
            None,
        ),
        CodeError::InvalidEmail => (
            StatusCode::BAD_REQUEST,
            40022,
            "Enter a valid email address.",
            None,
        ),
        CodeError::Locked(seconds) => (
            StatusCode::TOO_MANY_REQUESTS,
            42931,
            "Too many incorrect codes. Try again in one hour.",
            Some(seconds),
        ),
        CodeError::Limited(seconds) => (
            StatusCode::TOO_MANY_REQUESTS,
            42932,
            "Please wait before requesting another code.",
            Some(seconds),
        ),
        CodeError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            50321,
            "Email verification is temporarily unavailable. Try again later.",
            None,
        ),
    };
    let mut response = (
        status,
        Json(ApiResponse {
            code,
            message: message.into(),
            data: json!({"retryAfterSeconds":retry}),
        }),
    )
        .into_response();
    if let Some(retry) = retry {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&retry.to_string()).expect("integer header"),
        );
    }
    response
}
fn binding(user: &users::UserRow) -> String {
    format!("{}:{}", user.id, user.session_version)
}

pub async fn send_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<SendCodeRequest>,
) -> Response {
    let email = match normalize_email(&request.email) {
        Ok(email) => email,
        Err(error) => return code_error(error),
    };
    let bound = match request.purpose {
        Purpose::Register => "register".into(),
        Purpose::ResetPassword => match users::find_active_by_email(&state.db, &email).await {
            Ok(users) if users.len() == 1 => binding(&users[0]),
            Ok(_) => "no-eligible-account".into(),
            Err(_) => return web::unavailable(),
        },
    };
    // Every valid mailbox receives the same kind of challenge. A missing or
    // ambiguous account binds to an unusable identity, without timing/existence
    // differences in the send response. Strict quotas apply to all requests.
    let chinese = headers
        .get("x-app-language")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|lang| lang.starts_with("zh"));
    match state
        .email_codes
        .send(
            &email,
            request.purpose,
            &bound,
            super::client_ip(&state, Some(connect), &headers),
            chinese,
        )
        .await
    {
        Ok(()) => web::api_ok(json!({"retryAfterSeconds":60,"expiresInSeconds":600})),
        Err(error) => code_error(error),
    }
}

pub async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<ResetPasswordRequest>,
) -> Response {
    // Five wrong codes lock an address for everyone. Bound how many addresses one
    // source can burn through; the per-address lock still stops code guessing.
    match state
        .reset_password_rate_limiter
        .try_acquire(super::client_ip(&state, Some(connect), &headers))
        .await
    {
        RateLimitOutcome::Allowed => {}
        RateLimitOutcome::Limited {
            retry_after_seconds,
        } => {
            let seconds = u32::try_from(retry_after_seconds.max(1)).unwrap_or(u32::MAX);
            return code_error(CodeError::Limited(seconds));
        }
        RateLimitOutcome::Unavailable => return code_error(CodeError::Unavailable),
    }
    let email = match normalize_email(&request.email) {
        Ok(email) => email,
        Err(error) => return code_error(error),
    };
    if request.new_password.encode_utf16().count() < password::MIN_PASSWORD_UTF16_UNITS
        || !password::within_bcrypt_limit(&request.new_password)
    {
        return web::api_error(
            StatusCode::BAD_REQUEST,
            400,
            "Use at least 4 characters and no more than 72 UTF-8 bytes for your password.",
        );
    }
    let user = match users::find_active_by_email(&state.db, &email).await {
        Ok(mut users) if users.len() == 1 => users.pop(),
        Ok(_) => None,
        Err(_) => return web::unavailable(),
    };
    let bound = user
        .as_ref()
        .map(binding)
        .unwrap_or_else(|| "no-eligible-account".into());
    if let Err(error) = state
        .email_codes
        .consume(
            &email,
            Purpose::ResetPassword,
            &bound,
            &request.verification_code,
        )
        .await
    {
        return code_error(error);
    }
    let Some(user) = user else {
        return code_error(CodeError::Invalid);
    };
    let hash = match state.passwords.hash(request.new_password).await {
        Ok(hash) => hash,
        Err(_) => return web::unavailable(),
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return web::unavailable(),
    };
    match users::reset_password(&mut *tx, user.id, user.row_version, &hash).await {
        Ok(Some(_)) => {}
        Ok(None) => return code_error(CodeError::Invalid),
        Err(_) => return web::unavailable(),
    }
    if sqlx::query(
        "UPDATE users SET email_verified_at = COALESCE(email_verified_at, now()) WHERE id = $1",
    )
    .bind(user.id)
    .execute(&mut *tx)
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return web::unavailable();
    }
    // The DB session_version increment invalidates every old session, including
    // concurrent password-change transitions. Recovery never logs the user in.
    web::api_ok(json!(null))
}
