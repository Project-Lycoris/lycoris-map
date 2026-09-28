//! Axum routes, shared services, health checks, and middleware.
//!
//! Keep CORS outside request failures so allowed clients can read their errors.
//! The outer request log supplies a server-generated ID without logging secrets.
//! Body limits, timeouts, and write-origin checks apply across feature routes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use fred::clients::Client;
use fred::prelude::*;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tracing::Instrument;
use uuid::Uuid;

use crate::config::{Config, REQUEST_BODY_LIMIT_BYTES};
use crate::error::AppError;
use crate::media::{ImageStore, MediaService};
use crate::modules::markers::cache::MarkerCache;
use crate::modules::markers::repository::MarkerRepository;
use crate::modules::markers::service::MarkerService;
use crate::modules::markers::write::MarkerWriteService;
use crate::origin::enforce_write_origin;
use crate::password::PasswordHasher;
use crate::ratelimit::RegisterRateLimiter;
use crate::routes;
use crate::session::SessionStore;

/// `/health/ready` 单项依赖检查的超时；保证依赖卡住时可靠返回 503，
/// 而不会被全局请求超时先截断为 408。
const READY_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// 响应关联头名：每个响应都回填服务端生成的请求 ID，便于与日志关联。
const REQUEST_ID_HEADER: &str = "x-request-id";

/// 未匹配到任何路由模板时的固定占位；绝不回退到真实 URI（避免把 query/路径细节写进日志）。
const UNMATCHED_ROUTE: &str = "<unmatched>";

/// 应用共享状态。`Client` 与 `PgPool` 均为克隆廉价的句柄。
#[derive(Clone)]
pub struct AppState {
    pub email_codes: crate::email_verification::EmailCodes,
    pub db: PgPool,
    pub redis: Client,
    pub config: Arc<Config>,
    pub markers: MarkerService,
    pub markers_write: MarkerWriteService,
    pub session: SessionStore,
    pub passwords: PasswordHasher,
    pub rate_limiter: RegisterRateLimiter,
    /// 图片存储核心（头像/点位图片共用）。
    pub images: ImageStore,
    /// 媒体业务编排（头像、受控 `/uploads` 读取、点位图片提案）。
    pub media: MediaService,
}

impl AppState {
    /// 构造共享状态。
    ///
    /// 图片存储根目录在启动时创建并 canonicalize；失败返回明确错误，
    /// **不** `unwrap`/`panic`。`main` 在 `--migrate` 路径不会构造 `AppState`，
    /// 因此迁移不会初始化上传路径。
    ///
    /// 读取、写入与媒体三个服务共用同一个 `MarkerCache`（同一命名空间），
    /// 避免为 read/write/media 建出不一致的缓存实例。
    pub fn new(db: PgPool, redis: Client, config: Config) -> Result<Self, AppError> {
        let email_codes = crate::email_verification::EmailCodes::new(redis.clone(), &config)?;
        let marker_cache = MarkerCache::new(
            redis.clone(),
            config.marker_cache_enabled,
            config.marker_cache_namespace.clone(),
        );
        let markers = MarkerService::new(
            MarkerRepository::new(db.clone()),
            marker_cache.clone(),
            config.availability_zone,
        );
        let markers_write = MarkerWriteService::new(db.clone(), marker_cache.clone());
        let session = SessionStore::new(
            redis.clone(),
            config.session_namespace.clone(),
            config.session_ttl,
            config.redis_command_timeout,
        );
        let passwords = PasswordHasher::new(config.bcrypt_cost, config.password_max_concurrency);
        let rate_limiter = RegisterRateLimiter::new(
            redis.clone(),
            config.rate_limit_namespace.clone(),
            config.register_rate_limit_max,
            config.register_rate_limit_window,
            config.redis_command_timeout,
        );
        let images = ImageStore::new(&config.upload_dir, config.media_max_concurrency)?;
        let media = MediaService::new(db.clone(), images.clone(), marker_cache);
        Ok(Self {
            email_codes,
            db,
            redis,
            config: Arc::new(config),
            markers,
            markers_write,
            session,
            passwords,
            rate_limiter,
            images,
            media,
        })
    }
}

/// 组装 Router 与中间件。
pub fn build_router(state: AppState) -> Router {
    let cors = build_cors(&state.config.cors_allowed_origins);
    let request_timeout = state.config.request_timeout;
    let origin_state = state.clone();
    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .merge(crate::modules::markers::http::router())
        // Place mutations, bookmarks, moderation, and resumable image sessions.
        .merge(crate::modules::markers::write_http::router())
        .merge(crate::media::resumable::router())
        // Accounts and email verification.
        .route("/api/login", axum::routing::post(routes::auth::login))
        .route("/api/register", axum::routing::post(routes::auth::register))
        .route(
            "/api/auth/email-code",
            axum::routing::post(routes::email::send_code),
        )
        .route(
            "/api/auth/reset-password",
            axum::routing::post(routes::email::reset_password),
        )
        .route(
            "/api/me",
            get(routes::auth::me).patch(routes::auth::update_me),
        )
        .route(
            "/api/me/avatar",
            get(routes::avatar::me_avatar).post(routes::avatar::upload_avatar),
        )
        .route(
            "/api/users/{public_id}/avatar",
            get(routes::avatar::user_avatar),
        )
        .route(
            "/api/me/password",
            axum::routing::post(routes::auth::change_password),
        )
        .route("/api/logout", axum::routing::post(routes::auth::logout))
        // Admin verification and account management.
        .route(
            "/api/admin/verify",
            axum::routing::post(routes::admin::verify),
        )
        .route("/api/admin/users", get(routes::admin::list_users))
        .route(
            "/api/admin/users/{id}/reset-password",
            axum::routing::post(routes::admin::reset_password),
        )
        .route(
            "/api/admin/users/{id}",
            axum::routing::delete(routes::admin::delete_user),
        )
        .route(
            "/api/admin/users/{id}/restore",
            axum::routing::post(routes::admin::restore_user),
        )
        // Image moderation and the retired cleanup endpoint require AdminUser.
        .route(
            "/api/admin/markers/pending-images",
            get(routes::admin_markers::pending_images),
        )
        .route(
            "/api/admin/markers/image-proposals/{id}/approve",
            axum::routing::post(routes::admin_markers::approve_image_proposal),
        )
        .route(
            "/api/admin/markers/image-proposals/{id}/reject",
            axum::routing::post(routes::admin_markers::reject_image_proposal),
        )
        .route(
            "/api/admin/markers/cleanup-missing-images",
            axum::routing::post(routes::admin_markers::cleanup_missing_images),
        )
        // Keep media directories explicit: avatars are public, while marker images
        // resolve an optional viewer and check current place visibility.
        .route(
            "/uploads/avatars/{filename}",
            get(routes::uploads::serve_avatar),
        )
        .route(
            "/uploads/markers/{filename}",
            get(routes::uploads::serve_marker),
        )
        // 写请求来源校验对所有路由（含 login/register/logout 与 multipart）生效。
        .layer(middleware::from_fn_with_state(
            origin_state,
            enforce_write_origin,
        ))
        // multipart 上传显式覆盖 Axum 默认 2 MiB 为 8 MiB。
        .layer(axum::extract::DefaultBodyLimit::max(
            REQUEST_BODY_LIMIT_BYTES,
        ))
        // tower-http 标准全局请求体上限 8 MiB：已知 Content-Length 超限直接 413，
        // 未知长度的流式 body 由 `Limited` 在读取时抛 `LengthLimitError`（沿 source 链识别）。
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ))
        // 把 tower-http 的空体/文本 413 统一为 GlobalExceptionHandler 形状；JSON 提取器自己的
        // 结构化 413（application/json）原样保留，避免把损坏请求体误报为“图片太大”。
        .layer(middleware::from_fn(normalize_payload_too_large))
        // CORS 包住所有错误来源：限流/标准化后的 413 与其它响应都由它统一补齐
        // Access-Control-Allow-Origin/Credentials 与 Vary；非白名单不发 allow-origin。
        .layer(cors)
        // 访问日志/请求 ID 为最外层（包住 CORS）：普通成功、404 fallback 与全局 body limit
        // 413 都会生成请求 ID 并留下完成日志。`Router::layer` 在路由匹配后执行，仍能读到
        // `MatchedPath`（缺失时用固定占位，绝不记录真实 URI）；CORS 仍包住所有错误，故允许
        // 来源的错误响应都带跨域头。日志层只增补 `X-Request-ID`，不覆盖已有的响应头。
        .layer(middleware::from_fn(log_requests))
        .with_state(state)
}

fn build_cors(origins: &[axum::http::HeaderValue]) -> CorsLayer {
    CorsLayer::new()
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::AUTHORIZATION,
            HeaderName::from_static("x-app-language"),
        ])
        .allow_origin(AllowOrigin::list(origins.to_vec()))
        // 允许来源的浏览器可读取服务端回填的关联头，便于把前端错误关联到服务端日志。
        .expose_headers([HeaderName::from_static(REQUEST_ID_HEADER)])
}

/// 把 tower-http 全局请求体上限产生的 413（空体/文本）统一为 `GlobalExceptionHandler` 的
/// `ApiResponse{code:413,message:"上传文件过大，请选择 5MB 以内的图片",data:null}`。
///
/// 已是 `application/json` 的响应（如 JSON 提取器自己的 `请求体过大`）原样返回，
/// 不退化为空体、不覆盖其内容相关头；跨域头由更外层的 CORS 统一补齐。
async fn normalize_payload_too_large(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return response;
    }
    let already_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if already_json {
        return response;
    }
    crate::multipart::payload_too_large_response()
}

/// 访问日志与请求 ID 中间件（最外层，包住 CORS）。
///
/// 每个请求都由**服务端**生成一个新的 UUID 作为请求 ID：放入 tracing span，使 handler 与
/// 下游受控日志都落在同一 span 内；完成事件只记录请求 ID、方法、匹配路由模板、状态与耗时。
/// 明确**不**记录原始 URI/query、Cookie、请求体或任何用户内容；也**不**信任或回显客户端
/// 传入的 `X-Request-ID`。响应统一附 `X-Request-ID`（CORS 已 expose 该头）供关联。
///
/// 该层位于路由匹配之后、所有错误来源之外，因此普通成功、404 fallback 与全局请求体 413
/// 都会得到请求 ID 与完成日志；路由模板缺失时用固定占位，不回退到真实 URI。
async fn log_requests(matched: Option<MatchedPath>, request: Request, next: Next) -> Response {
    // 只取方法；不读 URI、query、Cookie 或 body。客户端提供的 X-Request-ID 被忽略。
    let method = request.method().clone();
    let route = matched
        .map(|path| path.as_str().to_string())
        .unwrap_or_else(|| UNMATCHED_ROUTE.to_string());
    let request_id = Uuid::new_v4().to_string();

    let span = tracing::info_span!(
        target: "lycoris_backend::http",
        "http.request",
        request_id = %request_id,
        %method,
        %route,
    );

    let started = Instant::now();
    // handler 及下游受控日志在该 span 中执行，自动携带 request_id/method/route。
    let mut response = next.run(request).instrument(span.clone()).await;
    let status = response.status();
    span.in_scope(|| {
        tracing::info!(
            target: "lycoris_backend::http",
            status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "request completed"
        );
    });

    // UUID 原文为 ASCII，`from_str` 不会失败；失败时保守跳过，不影响业务响应。
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    response
}

/// 进程存活探针：不依赖下游，恒返回 200。
async fn health_live() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// 就绪探针：并发检查 PG（`SELECT 1`）与 Redis（`PING`），各自带短超时；
/// 任一失败或超时返回 503。
async fn health_ready(State(state): State<AppState>) -> impl IntoResponse {
    let (db_ok, redis_ok) = tokio::join!(check_database(&state.db), check_redis(&state.redis));
    let ready = db_ok && redis_ok;

    let body: Value = json!({
        "status": if ready { "ok" } else { "unavailable" },
        "checks": {
            "postgres": if db_ok { "ok" } else { "down" },
            "redis": if redis_ok { "ok" } else { "down" },
        }
    });
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

async fn check_database(db: &PgPool) -> bool {
    matches!(
        tokio::time::timeout(READY_CHECK_TIMEOUT, sqlx::query("SELECT 1").fetch_one(db)).await,
        Ok(Ok(_))
    )
}

async fn check_redis(redis: &Client) -> bool {
    matches!(
        tokio::time::timeout(READY_CHECK_TIMEOUT, redis.ping::<()>(None)).await,
        Ok(Ok(()))
    )
}
