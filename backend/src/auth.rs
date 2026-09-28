//! 身份提取器：`OptionalUser`、`CurrentUser`、`AdminUser`。
//!
//! 每个需要身份的请求都从 Cookie 解析标识、从 Redis 读取类型化会话、并**从 PG 重新加载**
//! 当前账号，检查 `deleted`、`sessionVersion` 与角色。权限只看当前数据库：
//! 会话中的观测角色不授予任何权限。
//!
//! 并发安全：
//! - 版本失配时**不无条件删除**：先有界重读 Redis，给并发的版本推进（如改密保留当前会话）
//!   落地的时间；若版本已推进且仍有效，按新状态返回，绝不把普通并发请求变成虚假 401。
//!   确实陈旧时按读取快照 CAS 失效（key 存在且 userId/版本/观测角色一致才删），
//!   CAS 输给并发说明状态已变化，重读新状态；退出由 logout 无条件删除。
//! - 角色变化用 CAS 同步观测角色并清除二次验证：Redis 错误返回 503，绝不忽略后继续授权；
//!   CAS 输给并发同步或 logout 时重读，不会复活已退出会话。

use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::app::AppState;
use crate::session::{NewSession, SessionRecord};
use crate::users::{self, UserRow};
use crate::web;

/// 版本失配时的有界重读次数；超过后按快照 CAS 失效。
const MAX_STALE_WAITS: u32 = 4;
/// 单次身份加载的最大循环次数，防止病态循环。
const MAX_ITERATIONS: u32 = 10;

/// 已认证身份快照。含不透明标识，Debug 经过脱敏。
#[derive(Clone)]
pub struct Identity {
    /// 当前数据库账号（未删除）。
    pub user: UserRow,
    /// 不透明会话标识；仅用于本请求内的 Redis 操作，绝不记录。
    pub token: String,
    /// 观测到的会话身份（版本已与 DB 对齐，角色已同步）。
    pub session: NewSession,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("user", &self.user)
            .field("session", &self.session)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Identity {
    pub fn is_admin(&self) -> bool {
        self.user.is_admin()
    }
}

/// 提取失败时的响应。形状按 Java 契约分别选择。
///
/// 注意：`Forbidden` 是“已认证但角色不足”，真实 Java 走 Spring Boot 默认错误分派，
/// Forbidden returns `{timestamp,status,error,path}`; unauthorized uses the security envelope.
#[derive(Debug, Clone)]
pub enum AuthRejection {
    /// 未认证：安全入口固定 401 JSON。
    Unauthorized,
    /// 已认证但权限不足：Spring Boot 默认 403 JSON（携带请求 path）。
    Forbidden(String),
    /// 依赖故障：503。
    Unavailable,
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        match self {
            AuthRejection::Unauthorized => web::security_entry(),
            AuthRejection::Forbidden(path) => web::forbidden(&path),
            AuthRejection::Unavailable => web::unavailable(),
        }
    }
}

fn snapshot_of(record: &SessionRecord) -> NewSession {
    NewSession {
        user_id: record.user_id,
        session_version: record.session_version,
        role: record.role.clone(),
    }
}

async fn read_record(
    state: &AppState,
    token: &str,
) -> Result<Option<SessionRecord>, AuthRejection> {
    state
        .session
        .read(token)
        .await
        .map_err(|_| AuthRejection::Unavailable)
}

/// 按快照 CAS 失效；Redis 错误按 503。`now_ms` 供 Lua 判断改密 pending 是否仍有效。
async fn invalidate(
    state: &AppState,
    token: &str,
    snapshot: &NewSession,
    now_ms: i64,
) -> Result<bool, AuthRejection> {
    state
        .session
        .invalidate_if_matches(token, snapshot, now_ms)
        .await
        .map_err(|_| AuthRejection::Unavailable)
}

/// 加载身份；`None` 表示匿名（无 Cookie、无记录、账号停用或版本失配已清理）。
async fn load_identity(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<Identity>, AuthRejection> {
    let Some(token) = web::session_token(headers, &state.config.session_cookie_name) else {
        return Ok(None);
    };
    let Some(mut record) = read_record(state, &token).await? else {
        return Ok(None);
    };

    let mut stale_waits: u32 = 0;
    let mut iterations: u32 = 0;
    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            return Ok(None);
        }

        let snapshot = snapshot_of(&record);
        let user = match users::find_any_by_id(&state.db, record.user_id).await {
            Ok(user) => user,
            Err(_) => return Err(AuthRejection::Unavailable),
        };
        let now = chrono::Utc::now().timestamp_millis();

        let Some(user) = user else {
            // 账号不存在：按快照 CAS 失效；CAS 输给并发则重读，不误删新记录。
            if invalidate(state, &token, &snapshot, now).await? {
                return Ok(None);
            }
            record = match read_record(state, &token).await? {
                Some(record) => record,
                None => return Ok(None),
            };
            continue;
        };

        if user.deleted {
            if invalidate(state, &token, &snapshot, now).await? {
                return Ok(None);
            }
            record = match read_record(state, &token).await? {
                Some(record) => record,
                None => return Ok(None),
            };
            continue;
        }

        if record.session_version != user.session_version {
            // 改密已写 PG 但 Redis 尚未推进：存在有效 pending 时这是正常转换窗口，
            // 有界重读等待推进；仍未结束则 503，绝不 401 或删除当前会话。
            if record.pending_active(now) {
                if stale_waits < MAX_STALE_WAITS {
                    stale_waits += 1;
                    tokio::time::sleep(Duration::from_millis(u64::from(stale_waits) * 5)).await;
                    record = match read_record(state, &token).await? {
                        Some(record) => record,
                        None => return Ok(None),
                    };
                    continue;
                }
                return Err(AuthRejection::Unavailable);
            }
            if stale_waits < MAX_STALE_WAITS {
                stale_waits += 1;
                tokio::time::sleep(Duration::from_millis(u64::from(stale_waits) * 5)).await;
                record = match read_record(state, &token).await? {
                    Some(record) => record,
                    None => return Ok(None),
                };
                continue;
            }
            // 确认陈旧：按快照 CAS 失效；Lua 同时拒绝删除仍有有效 pending 的记录，
            // 若并发已推进/新 pending 则 CAS 失败并重读新状态。
            if invalidate(state, &token, &snapshot, now).await? {
                return Ok(None);
            }
            record = match read_record(state, &token).await? {
                Some(record) => record,
                None => return Ok(None),
            };
            continue;
        }

        // 版本有效：处理观测角色变化。
        if record.role != user.role {
            match state.session.sync_role(&token, &snapshot, &user.role).await {
                Ok(true) => {
                    record.role = user.role.clone();
                    // 角色变化即清除本次返回的二次验证状态，绝不沿用旧记录的 second_at。
                    record.second_at = None;
                }
                Ok(false) => {
                    // CAS 输给并发同步或 logout：有界重读，退出则为匿名。
                    record = match read_record(state, &token).await? {
                        Some(record) => record,
                        None => return Ok(None),
                    };
                    continue;
                }
                // Redis 故障不得忽略后继续授予权限。
                Err(_) => return Err(AuthRejection::Unavailable),
            }
        }

        return Ok(Some(Identity {
            user,
            token,
            session: snapshot_of(&record),
        }));
    }
}

/// 匿名或已认证；仅依赖故障时为 503。
pub struct OptionalUser(pub Option<Identity>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        load_identity(state, &parts.headers).await.map(OptionalUser)
    }
}

/// 必须已认证。
pub struct CurrentUser(pub Identity);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match load_identity(state, &parts.headers).await? {
            Some(identity) => Ok(CurrentUser(identity)),
            None => Err(AuthRejection::Unauthorized),
        }
    }
}

/// 必须是数据库当前角色为 `ADMIN` 的登录用户（不要求二次验证）。
pub struct AdminUser(pub Identity);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match load_identity(state, &parts.headers).await? {
            Some(identity) if identity.is_admin() => Ok(AdminUser(identity)),
            Some(_) => Err(AuthRejection::Forbidden(parts.uri.path().to_string())),
            None => Err(AuthRejection::Unauthorized),
        }
    }
}
