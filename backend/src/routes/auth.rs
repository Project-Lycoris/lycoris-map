//! AuthController 对应接口：登录、注册、me、资料、改密、退出。

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::Value;

use crate::app::AppState;
use crate::auth::CurrentUser;
use crate::dto::{ChangePasswordRequest, LoginRequest, RegisterRequest, UpdateProfileRequest};
use crate::password::{self, HashError};
use crate::ratelimit::RateLimitOutcome;
use crate::session::{NewSession, TransitionState};
use crate::users::{self, UserRow};
use crate::web::{self, JsonBody};
use uuid::Uuid;

/// 改密转换的 pending 有界期限（60 秒）；超时后按无 pending 处理。
const PASSWORD_CHANGE_PENDING_MS: i64 = 60_000;

/// 登录失败统一使用的通用凭据错误（不区分账号不存在/密码错误/重复账号）。
fn invalid_credentials() -> Response {
    web::api_error(
        StatusCode::UNAUTHORIZED,
        4001,
        "Invalid username or password",
    )
}

fn register_rejected() -> Response {
    web::api_error(
        StatusCode::BAD_REQUEST,
        4002,
        "Username or email already exists",
    )
}

/// POST /api/login
pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonBody(request): JsonBody<LoginRequest>,
) -> Response {
    let (Some(identity), Some(password)) = (request.username, request.password) else {
        return invalid_credentials();
    };
    let identity = identity.trim().to_string();
    if identity.is_empty() {
        return invalid_credentials();
    }

    let candidates = users::find_active_by_identity(&state.db, &identity).await;
    // 历史库可能重复；匹配到多个账号时按通用凭据失败处理，绝不选择首个账号。
    let user = match candidates {
        Ok(list) if list.len() == 1 => list.into_iter().next().expect("已确认长度为一"),
        Ok(_) => return invalid_credentials(),
        Err(_) => return web::unavailable(),
    };

    let stored = user.password.clone().unwrap_or_default();
    if !state.passwords.matches(&password, &stored).await {
        return invalid_credentials();
    }

    // 历史明文仅在存储值无编码前缀时才可能命中；成功后条件升级，避免覆盖并发重置。
    if !password::is_encoded(&stored) {
        let new_hash = match state.passwords.hash(password.clone()).await {
            Ok(hash) => hash,
            Err(HashError::TooLong) => return invalid_credentials(),
            Err(_) => return web::unavailable(),
        };
        match users::upgrade_legacy_password(
            &state.db,
            user.id,
            &stored,
            user.row_version,
            &new_hash,
        )
        .await
        {
            Ok(true) => {}
            // 并发重置/改密赢了：不基于旧明文授权。
            Ok(false) => return invalid_credentials(),
            Err(_) => return web::unavailable(),
        }
    }

    let old_token = web::session_token(&headers, &state.config.session_cookie_name);
    let session = NewSession {
        user_id: user.id,
        session_version: user.session_version,
        role: user.role.clone(),
    };
    match state.session.create(&session, old_token.as_deref()).await {
        Ok(token) => {
            let response = web::api_ok(user_response_or_null(&user));
            web::set_cookie(response, &state.config, &token)
        }
        Err(_) => web::unavailable(),
    }
}

/// POST /api/register
pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: ConnectInfo<SocketAddr>,
    JsonBody(request): JsonBody<RegisterRequest>,
) -> Response {
    // 蜜罐字段非空即拒绝（在限流之前，与 Java 顺序一致）。
    if request
        .website
        .as_deref()
        .is_some_and(|website| !website.trim().is_empty())
    {
        return web::api_error(StatusCode::BAD_REQUEST, 4004, "注册请求无效");
    }

    let ip = super::client_ip(&state, Some(connect), &headers);
    match state.rate_limiter.try_acquire(ip).await {
        RateLimitOutcome::Allowed => {}
        RateLimitOutcome::Limited => {
            return web::api_error(
                StatusCode::TOO_MANY_REQUESTS,
                429,
                "请求过于频繁，请稍后再试",
            );
        }
        RateLimitOutcome::Unavailable => return web::unavailable(),
    }

    let username = request.username.unwrap_or_default().trim().to_string();
    let email = match crate::email_verification::normalize_email(&request.email.unwrap_or_default())
    {
        Ok(email) => email,
        Err(error) => return super::email::code_error(error),
    };
    let password = request.password.unwrap_or_default();
    let nickname = match request.nickname {
        Some(raw) if !raw.trim().is_empty() => raw.trim().to_string(),
        _ => username.clone(),
    };

    if username.is_empty()
        || email.is_empty()
        || password.encode_utf16().count() < password::MIN_PASSWORD_UTF16_UNITS
    {
        return register_rejected();
    }
    // 按真实 schema 约束在应用层返回 400，避免落到 PG 变成 500。
    if !fits(&username, users::USERNAME_MAX)
        || !fits(&email, 255)
        || !fits(&nickname, 255)
        || !password::within_bcrypt_limit(&password)
    {
        return register_rejected();
    }

    if let Err(error) = state
        .email_codes
        .consume(
            &email,
            crate::email_verification::Purpose::Register,
            "register",
            request.verification_code.as_deref().unwrap_or_default(),
        )
        .await
    {
        return super::email::code_error(error);
    }

    // 哈希在持锁前完成，缩短 advisory lock 临界区。
    let password_hash = match state.passwords.hash(password).await {
        Ok(hash) => hash,
        Err(HashError::TooLong) => return register_rejected(),
        Err(_) => return web::unavailable(),
    };

    let user =
        match insert_registered_user(&state, &username, &nickname, &email, &password_hash).await {
            Ok(Some(user)) => user,
            Ok(None) => return register_rejected(),
            Err(()) => return web::unavailable(),
        };

    // PG 已提交后再创建会话；会话创建失败不能谎称回滚，返回明确的“账号已创建”。
    let session = NewSession {
        user_id: user.id,
        session_version: user.session_version,
        role: user.role.clone(),
    };
    // 注册等同登录：携带请求中的旧标识，原子替换，避免注册前的旧会话继续存活。
    let old_token = web::session_token(&headers, &state.config.session_cookie_name);
    match state.session.create(&session, old_token.as_deref()).await {
        Ok(token) => {
            let response = web::api_ok(user_response_or_null(&user));
            web::set_cookie(response, &state.config, &token)
        }
        Err(_) => web::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            503,
            "账号已创建，请稍后登录",
        ),
    }
}

/// 在 advisory lock 事务内重新检查用户名与规范化邮箱后插入；重复返回 `Ok(None)`。
async fn insert_registered_user(
    state: &AppState,
    username: &str,
    nickname: &str,
    email: &str,
    password_hash: &str,
) -> Result<Option<UserRow>, ()> {
    let mut tx = state.db.begin().await.map_err(|_| ())?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(users::REGISTER_ADVISORY_LOCK_KEY)
        .execute(&mut *tx)
        .await
        .map_err(|_| ())?;

    if users::username_exists_any(&mut *tx, username)
        .await
        .map_err(|_| ())?
        || users::email_exists_any(&mut *tx, email)
            .await
            .map_err(|_| ())?
    {
        // 重复：事务随作用域回滚。
        return Ok(None);
    }

    let user = users::insert_user(&mut *tx, username, nickname, email, password_hash)
        .await
        .map_err(|_| ())?;
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| ())?;
    tx.commit().await.map_err(|_| ())?;
    Ok(Some(user))
}

/// GET /api/me
pub async fn me(user: CurrentUser) -> Response {
    web::api_ok(user_response_or_null(&user.0.user))
}

/// PATCH /api/me
pub async fn update_me(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(request): JsonBody<UpdateProfileRequest>,
) -> Response {
    let current = &user.0.user;
    let nickname = match request.nickname {
        Some(raw) => {
            let trimmed = raw.trim();
            Some(if trimmed.is_empty() {
                current.username_or_empty().to_string()
            } else {
                trimmed.to_string()
            })
        }
        None => current.nickname.clone(),
    };
    let pronouns = normalize_optional(request.pronouns, current.pronouns.clone());
    let signature = normalize_optional(request.signature, current.signature.clone());

    if nickname.as_deref().is_some_and(|value| !fits(value, 255))
        || pronouns.as_deref().is_some_and(|value| !fits(value, 64))
        || signature.as_deref().is_some_and(|value| !fits(value, 200))
    {
        return web::api_error(StatusCode::BAD_REQUEST, 400, "资料字段过长");
    }

    match users::update_profile(
        &state.db,
        current.id,
        current.row_version,
        nickname.as_deref(),
        pronouns.as_deref(),
        signature.as_deref(),
    )
    .await
    {
        Ok(Some(updated)) => web::api_ok(user_response_or_null(&updated)),
        Ok(None) => optimistic_conflict(),
        Err(_) => web::unavailable(),
    }
}

/// POST /api/me/password
pub async fn change_password(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(request): JsonBody<ChangePasswordRequest>,
) -> Response {
    let (Some(old_password), Some(new_password)) = (request.old_password, request.new_password)
    else {
        return web::api_error(StatusCode::BAD_REQUEST, 400, "缺少参数");
    };
    if password::utf16_len(&new_password) < password::MIN_PASSWORD_UTF16_UNITS {
        return web::api_error(StatusCode::BAD_REQUEST, 400, "原密码错误或新密码不合法");
    }

    let current = &user.0.user;
    let stored = current.password.clone().unwrap_or_default();
    if !state.passwords.matches(&old_password, &stored).await {
        return web::api_error(StatusCode::BAD_REQUEST, 400, "原密码错误或新密码不合法");
    }
    let new_hash = match state.passwords.hash(new_password).await {
        Ok(hash) => hash,
        Err(HashError::TooLong) => {
            return web::api_error(StatusCode::BAD_REQUEST, 400, "原密码错误或新密码不合法");
        }
        Err(_) => return web::unavailable(),
    };

    // 写 PG 之前先建立短期 Redis 转换标记：Redis 不可用则不写 PG；
    // 已有进行中的转换返回 409，避免并发改密互相覆盖。
    let nonce = Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let pending_until = now.saturating_add(PASSWORD_CHANGE_PENDING_MS);
    match state
        .session
        .begin_password_change(&user.0.token, &user.0.session, &nonce, pending_until, now)
        .await
    {
        Ok(TransitionState::Marked) => {}
        Ok(TransitionState::AlreadyPending) => {
            return web::api_error(StatusCode::CONFLICT, 409, "密码修改正在进行，请稍后重试");
        }
        // 并发退出已删除会话：不写 PG。
        Ok(TransitionState::MissingOrMismatch) => return web::security_entry(),
        Err(_) => return web::unavailable(),
    }

    let updated = users::change_password(
        &state.db,
        current.id,
        &stored,
        current.row_version,
        current.session_version,
        &new_hash,
    )
    .await;
    match updated {
        Ok(Some(version)) => {
            // PG 已提交：以 nonce 原子推进版本、清 secondAt 与 pending。
            let new_session_version = version.session_version;
            match state
                .session
                .complete_password_change(
                    &user.0.token,
                    &user.0.session,
                    &nonce,
                    new_session_version,
                )
                .await
            {
                Ok(true) => web::api_ok(Value::Null),
                // 退出赢了或 Redis 故障：PG 已提交，不能假称回滚；清 Cookie 并提示重新登录。
                Ok(false) | Err(_) => {
                    tracing::warn!(
                        target: "lycoris_backend::auth",
                        "改密已提交但当前会话推进失败，用户需重新登录"
                    );
                    let response = web::api_ok(Value::Null);
                    web::clear_cookie(response, &state.config)
                }
            }
        }
        Ok(None) => {
            // PG 明确版本冲突：按 nonce 清除 pending，用户可稍后重试。
            let _ = state
                .session
                .cancel_password_change(&user.0.token, &user.0.session, &nonce)
                .await;
            optimistic_conflict()
        }
        // 结果不确定：保留 pending 由其自行过期，绝不复活会话。
        Err(_) => web::unavailable(),
    }
}

/// POST /api/logout
pub async fn logout(State(state): State<AppState>, user: CurrentUser) -> Response {
    // Redis 删除失败不能声称退出成功：logout 不改 PG.sessionVersion，旧 token 仍可用。
    match state.session.delete(&user.0.token).await {
        Ok(()) => {
            let response = web::api_ok(Value::Null);
            web::clear_cookie(response, &state.config)
        }
        Err(_) => web::unavailable(),
    }
}

fn optimistic_conflict() -> Response {
    web::api_error(StatusCode::CONFLICT, 409, "数据已更新，请刷新后重试")
}

/// `nickname` 之外的可选字段：`null` 保持原值，非空 trim 后写入，空白置 `null`。
fn normalize_optional(incoming: Option<String>, existing: Option<String>) -> Option<String> {
    match incoming {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => existing,
    }
}

fn fits(value: &str, max_chars: usize) -> bool {
    value.chars().count() <= max_chars
}

fn user_response_or_null(user: &UserRow) -> Value {
    crate::dto::user_response(user)
}
