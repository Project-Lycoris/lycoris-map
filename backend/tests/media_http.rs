//! 阶段 2/3 集成 HTTP 测试：头像 3 路由、受控 `/uploads`、私有点位 detail，以及阶段 3 的
//! 点位图片上传与管理员图片提案/清理 5 路由。
//!
//! 全部使用真实 PG / Redis / 临时上传目录；复用 `common` 的临时库与回环校验，
//! 不写真实 `uploads/`，退出即清理。

mod common;

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use common::{TempDatabase, connect_redis, test_redis_url, unreachable_redis};
use fred::clients::Client;
use futures_util::StreamExt;
use futures_util::stream;
use image::{ExtendedColorType, ImageEncoder};
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::modules::markers::localization::source_hash_components;
use lycoris_backend::password::PasswordHasher;
use sqlx::PgPool;
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;

const COOKIE_NAME: &str = "LYCORIS_SESSION";
const ALLOWED_ORIGIN: &str = "https://app.example.com";

// ---------------------------------------------------------------------------
// 测试环境
// ---------------------------------------------------------------------------

struct TestEnv {
    _temp: TempDatabase,
    pool: PgPool,
    upload: TempDir,
    config: Config,
    state: AppState,
    router: Router,
}

impl TestEnv {
    async fn new() -> Self {
        let (temp, pool) = TempDatabase::create_migrated().await;
        let redis = connect_redis().await;
        let unique = Uuid::new_v4().simple().to_string();
        let mut config = Config::new(temp.url(), test_redis_url());
        config.session_namespace = format!("lycoris:test:{unique}:session");
        config.rate_limit_namespace = format!("lycoris:test:{unique}:ratelimit");
        config.marker_cache_namespace = format!("lycoris:test:{unique}:marker");
        // 测试统一 cost 4。
        config.bcrypt_cost = 4;
        config.email_verification_secret = Some("synthetic-email-secret-at-least-32-bytes".into());
        config.write_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
        config.cors_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
        let upload = TempDir::new().expect("创建临时上传目录失败");
        config.upload_dir = upload.path().to_path_buf();
        let state = AppState::new(pool.clone(), redis, config.clone()).expect("构造 AppState 失败");
        let router = build_router(state.clone());
        Self {
            _temp: temp,
            pool,
            upload,
            config,
            state,
            router,
        }
    }

    fn upload_root(&self) -> &std::path::Path {
        self.upload.path()
    }

    /// 用指定 Redis 客户端重建 Router（同一临时库与上传根），用于依赖故障/匿名读取用例。
    fn router_with_redis(&self, redis: Client) -> Router {
        build_router(
            AppState::new(self.pool.clone(), redis, self.config.clone())
                .expect("构造 AppState 失败"),
        )
    }
}

// ---------------------------------------------------------------------------
// 请求辅助
// ---------------------------------------------------------------------------

struct Call {
    method: Method,
    uri: String,
    content_type: Option<String>,
    body: Vec<u8>,
    /// 需要注入错误/流式 body 时使用；非 `None` 时覆盖 `body`。
    raw_body: Option<Body>,
    cookie: Option<String>,
    headers: Vec<(String, String)>,
}

impl Call {
    fn new(method: Method, uri: &str) -> Self {
        Self {
            method,
            uri: uri.to_string(),
            content_type: None,
            body: Vec::new(),
            raw_body: None,
            cookie: None,
            headers: Vec::new(),
        }
    }

    fn multipart(uri: &str, boundary: &str, body: Vec<u8>) -> Self {
        let mut call = Self::new(Method::POST, uri);
        call.content_type = Some(format!("multipart/form-data; boundary={boundary}"));
        call.body = body;
        call
    }

    fn json(uri: &str, body: serde_json::Value) -> Self {
        let mut call = Self::new(Method::POST, uri);
        call.content_type = Some("application/json".to_string());
        call.body = serde_json::to_vec(&body).expect("序列化 JSON 失败");
        call
    }

    fn raw(method: Method, uri: &str, content_type: &str, body: Body) -> Self {
        let mut call = Self::new(method, uri);
        call.content_type = Some(content_type.to_string());
        call.raw_body = Some(body);
        call
    }

    fn cookie(mut self, token: impl Into<String>) -> Self {
        self.cookie = Some(token.into());
        self
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    async fn from(response: Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体失败")
            .to_vec();
        Self {
            status,
            headers,
            body,
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: header::HeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn session_cookie(&self) -> Option<String> {
        for value in self.headers.get_all(header::SET_COOKIE) {
            let raw = value.to_str().ok()?;
            let first = raw.split(';').next().unwrap_or_default();
            if let Some(rest) = first.strip_prefix(&format!("{COOKIE_NAME}=")) {
                return Some(rest.to_string());
            }
        }
        None
    }
}

async fn send(router: &Router, call: Call) -> Resp {
    let mut builder = Request::builder().method(call.method).uri(call.uri);
    if let Some(content_type) = &call.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let body = call.raw_body.unwrap_or_else(|| Body::from(call.body));
    let mut req = builder.body(body).expect("构造请求失败");
    if let Some(token) = call.cookie {
        req.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE_NAME}={token}")).expect("Cookie 非法"),
        );
    }
    for (name, value) in call.headers {
        req.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).expect("请求头名非法"),
            HeaderValue::from_str(&value).expect("请求头值非法"),
        );
    }
    req.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::from([127, 0, 0, 1]),
        51234,
    )));
    Resp::from(router.clone().oneshot(req).await.expect("调用路由失败")).await
}

// ---------------------------------------------------------------------------
// 测试数据辅助
// ---------------------------------------------------------------------------

fn rgba_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([(x * 7) as u8, (y * 11) as u8, 200, 255])
    });
    let mut output = Vec::new();
    image::codecs::png::PngEncoder::new(&mut output)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
        .expect("编码测试 PNG 失败");
    output
}

fn rgb_jpeg(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 5) as u8, (y * 3) as u8, 120])
    });
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut output)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .expect("编码测试 JPEG 失败");
    output
}

struct Part<'a> {
    name: &'a str,
    filename: Option<&'a str>,
    content_type: Option<&'a str>,
    data: &'a [u8],
}

fn file_part<'a>(
    name: &'a str,
    filename: &'a str,
    content_type: &'a str,
    data: &'a [u8],
) -> Part<'a> {
    Part {
        name,
        filename: Some(filename),
        content_type: Some(content_type),
        data,
    }
}

fn field_part<'a>(name: &'a str, data: &'a [u8]) -> Part<'a> {
    Part {
        name,
        filename: None,
        content_type: None,
        data,
    }
}

/// 按给定顺序拼接 multipart/form-data 请求体（保留字段顺序，便于测试重复/尾随字段）。
fn multipart_body(boundary: &str, parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match part.filename {
            Some(filename) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{filename}\"\r\n",
                    part.name
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{}\"\r\n", part.name).as_bytes(),
            ),
        }
        if let Some(content_type) = part.content_type {
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(part.data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

async fn register(env: &TestEnv, username: &str) -> (String, String) {
    use lycoris_backend::email_verification::Purpose;
    let email = format!("{username}@example.com");
    let nonce = env
        .state
        .email_codes
        .reserve(
            &email,
            Purpose::Register,
            "register",
            "127.0.0.1".parse().unwrap(),
            "123456",
        )
        .await
        .unwrap();
    env.state
        .email_codes
        .finish(&email, Purpose::Register, &nonce, true)
        .await
        .unwrap();
    let response = send(
        &env.router,
        Call::json(
            "/api/register",
            serde_json::json!({
                "username": username,
                "nickname": username,
                "email": email,
                "verificationCode": "123456",
                "password": "test-password",
                "website": "",
            }),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "注册应成功");
    let cookie = response.session_cookie().expect("注册应返回会话 Cookie");
    let public_id = response.json()["data"]["publicId"]
        .as_str()
        .expect("缺少 publicId")
        .to_string();
    (cookie, public_id)
}

async fn login(env: &TestEnv, username: &str) -> String {
    let response = send(
        &env.router,
        Call::json(
            "/api/login",
            serde_json::json!({ "username": username, "password": "test-password" }),
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "登录应成功");
    response.session_cookie().expect("登录应返回会话 Cookie")
}

async fn insert_admin(env: &TestEnv, username: &str) -> String {
    let hash = PasswordHasher::new(4, 1)
        .hash("test-password".to_string())
        .await
        .expect("生成测试哈希失败");
    let public_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (public_id, username, nickname, email, password, role, deleted, \
         session_version, row_version) VALUES ($1, $2, $2, $3, $4, 'ADMIN', false, 0, 0)",
    )
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.com"))
    .bind(hash)
    .execute(&env.pool)
    .await
    .expect("插入管理员失败");
    login(env, username).await
}

/// Log in with an ordinary administrator account.
async fn login_admin(env: &TestEnv, username: &str) -> String {
    insert_admin(env, username).await
}

async fn insert_image_proposal(
    env: &TestEnv,
    marker_id: i64,
    marker_title: &str,
    proposer_username: &str,
    proposer_public_id: &str,
    image_url: &str,
    status: &str,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO marker_image_proposals
            (marker_id, marker_title, proposer_username, proposer_public_id, image_url, status, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, now())
         RETURNING id",
    )
    .bind(marker_id)
    .bind(marker_title)
    .bind(proposer_username)
    .bind(proposer_public_id)
    .bind(image_url)
    .bind(status)
    .fetch_one(&env.pool)
    .await
    .expect("插入图片提案失败")
}

async fn insert_translation(
    env: &TestEnv,
    marker_id: i64,
    language: &str,
    title: &str,
    hash: &str,
) {
    sqlx::query(
        "INSERT INTO map_marker_translations (marker_id, language, title, description, source_hash)
         VALUES ($1, $2, $3, NULL, $4)",
    )
    .bind(marker_id)
    .bind(language)
    .bind(title)
    .bind(hash)
    .execute(&env.pool)
    .await
    .expect("插入译文失败");
}

async fn marker_row_state(env: &TestEnv, marker_id: i64) -> (i64, Option<String>) {
    sqlx::query_as("SELECT version, mark_image FROM map_markers WHERE id = $1")
        .bind(marker_id)
        .fetch_one(&env.pool)
        .await
        .expect("读取点位图片状态失败")
}

async fn proposal_row_state(env: &TestEnv, proposal_id: i64) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, reviewed_by FROM marker_image_proposals WHERE id = $1")
        .bind(proposal_id)
        .fetch_one(&env.pool)
        .await
        .expect("读取提案状态失败")
}

async fn insert_marker(
    env: &TestEnv,
    title: &str,
    is_public: bool,
    review_status: &str,
    owner_public_id: &str,
    mark_image: Option<&str>,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            open_time_start, open_time_end, review_status, username, user_public_id,
            mark_image, last_edited_by_owner, version, created_at, updated_at)
         VALUES (31.2, 121.4, 'accessible_toilet', $1, NULL, 'zh', $2, true,
                 NULL, NULL, $3, 'seed-user', $4, $5, true, 0, now(), now())
         RETURNING id",
    )
    .bind(title)
    .bind(is_public)
    .bind(review_status)
    .bind(owner_public_id)
    .bind(mark_image)
    .fetch_one(&env.pool)
    .await
    .expect("插入点位失败")
}

fn write_marker_file(env: &TestEnv, filename: &str, bytes: &[u8]) {
    let dir = env.upload_root().join("markers");
    std::fs::create_dir_all(&dir).expect("创建 markers 目录失败");
    std::fs::write(dir.join(filename), bytes).expect("写入点位图片失败");
}

// ---------------------------------------------------------------------------
// 私有点位 detail
// ---------------------------------------------------------------------------

#[tokio::test]
async fn private_marker_detail_uses_real_viewer() {
    let env = TestEnv::new().await;
    let (owner_cookie, owner_public_id) = register(&env, "detail-owner").await;
    let (other_cookie, _) = register(&env, "detail-other").await;
    let admin_cookie = insert_admin(&env, "detail-admin").await;

    let private = insert_marker(&env, "私有待审", false, "PENDING", &owner_public_id, None).await;
    let public = insert_marker(&env, "公开已审", true, "APPROVED", &owner_public_id, None).await;

    // 匿名：私有 404、公开 200。
    let anon_private = send(
        &env.router,
        Call::new(Method::GET, &format!("/api/markers/{private}")),
    )
    .await;
    assert_eq!(anon_private.status, StatusCode::NOT_FOUND);
    assert!(anon_private.body.is_empty(), "私有 404 应为空体");

    let anon_public = send(
        &env.router,
        Call::new(Method::GET, &format!("/api/markers/{public}")),
    )
    .await;
    assert_eq!(anon_public.status, StatusCode::OK);
    assert_eq!(anon_public.json()["id"], public);

    // 属主与管理员可见私有；他人仍 404。
    for (label, cookie) in [("owner", &owner_cookie), ("admin", &admin_cookie)] {
        let response = send(
            &env.router,
            Call::new(Method::GET, &format!("/api/markers/{private}")).cookie(cookie.clone()),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "{label} 应可见私有点位");
        assert_eq!(response.json()["id"], private);
        assert_eq!(response.json()["reviewStatus"], "PENDING");
    }
    let other = send(
        &env.router,
        Call::new(Method::GET, &format!("/api/markers/{private}")).cookie(other_cookie),
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "非属主仍应 404");
}

// ---------------------------------------------------------------------------
// 头像全链路
// ---------------------------------------------------------------------------

#[tokio::test]
async fn avatar_upload_and_read_full_chain() {
    let env = TestEnv::new().await;
    let (cookie, public_id) = register(&env, "avatar-user").await;

    // 未登录读取：401。
    let unauth = send(&env.router, Call::new(Method::GET, "/api/me/avatar")).await;
    assert_eq!(unauth.status, StatusCode::UNAUTHORIZED);

    // 无头像：404 空体。
    let none = send(
        &env.router,
        Call::new(Method::GET, "/api/me/avatar").cookie(cookie.clone()),
    )
    .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
    assert!(none.body.is_empty());

    // 上传 PNG（带 alpha → 重编码 PNG）。浏览器场景显式带允许的 Origin。
    let boundary = "lycoris-avatar-boundary";
    let png = rgba_png(4, 3);
    let body = multipart_body(boundary, &[file_part("file", "me.png", "image/png", &png)]);
    let uploaded = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body)
            .cookie(cookie.clone())
            .header("origin", ALLOWED_ORIGIN),
    )
    .await;
    assert_eq!(uploaded.status, StatusCode::OK, "上传应成功");
    let json = uploaded.json();
    assert_eq!(json["code"], 0);
    assert_eq!(json["message"], "ok");
    for key in [
        "publicId",
        "username",
        "nickname",
        "email",
        "avatarUrl",
        "pronouns",
        "signature",
    ] {
        assert!(
            json["data"].get(key).is_some(),
            "UserResponse 缺少字段 {key}"
        );
    }
    assert_eq!(json["data"]["publicId"], public_id);
    let avatar_url = json["data"]["avatarUrl"]
        .as_str()
        .expect("缺少 avatarUrl")
        .to_string();
    assert!(
        avatar_url.starts_with(&format!("/uploads/avatars/avatar-{public_id}-")),
        "头像 URL 应由前缀 avatar-<publicId>- 构成，实际 {avatar_url}"
    );
    assert!(
        avatar_url.ends_with(".png"),
        "PNG 上传应存为 png：{avatar_url}"
    );

    let filename = avatar_url
        .strip_prefix("/uploads/avatars/")
        .expect("头像 URL 前缀错误");
    let stored = std::fs::read(env.upload_root().join("avatars").join(filename))
        .expect("重编码后的头像文件应存在");

    // me/avatar：MIME、缓存头、nosniff、Content-Length、流式字节。
    let me = send(
        &env.router,
        Call::new(Method::GET, "/api/me/avatar").cookie(cookie.clone()),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.header(header::CONTENT_TYPE), Some("image/png"));
    assert_eq!(
        me.header(header::CACHE_CONTROL),
        Some("public, max-age=600")
    );
    assert_eq!(me.header(header::X_CONTENT_TYPE_OPTIONS), Some("nosniff"));
    let expected_len = stored.len().to_string();
    assert_eq!(
        me.header(header::CONTENT_LENGTH),
        Some(expected_len.as_str())
    );
    assert_eq!(me.body, stored);

    // 公共 ID 头像：匿名可读，同样字节。
    let public = send(
        &env.router,
        Call::new(Method::GET, &format!("/api/users/{public_id}/avatar")),
    )
    .await;
    assert_eq!(public.status, StatusCode::OK);
    assert_eq!(public.header(header::CONTENT_TYPE), Some("image/png"));
    assert_eq!(public.body, stored);

    // /uploads/avatars 匿名可读，no-store + nosniff。
    let uploads = send(
        &env.router,
        Call::new(Method::GET, &format!("/uploads/avatars/{filename}")),
    )
    .await;
    assert_eq!(uploads.status, StatusCode::OK);
    assert_eq!(uploads.header(header::CONTENT_TYPE), Some("image/png"));
    assert_eq!(uploads.header(header::CACHE_CONTROL), Some("no-store"));
    assert_eq!(
        uploads.header(header::X_CONTENT_TYPE_OPTIONS),
        Some("nosniff")
    );
    assert_eq!(uploads.body, stored);

    // 换成无 alpha 的 JPEG：MIME 应为 image/jpeg。
    let jpeg = rgb_jpeg(4, 3);
    let body = multipart_body(
        boundary,
        &[file_part("file", "me.jpg", "image/jpeg", &jpeg)],
    );
    let uploaded = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(uploaded.status, StatusCode::OK);
    let me = send(
        &env.router,
        Call::new(Method::GET, "/api/me/avatar").cookie(cookie.clone()),
    )
    .await;
    assert_eq!(me.header(header::CONTENT_TYPE), Some("image/jpeg"));
    assert!(!me.body.is_empty());
}

#[tokio::test]
async fn avatar_reads_404_for_missing_deleted_and_illegal_references() {
    let env = TestEnv::new().await;
    let (cookie, public_id) = register(&env, "avatar-missing").await;

    // 非法引用：非 /uploads/avatars/ 与路径穿越都按无头像处理。
    for illegal in [
        "http://evil.example/avatar.png",
        "/uploads/avatars/../secret.png",
        "/uploads/markers/whatever.png",
    ] {
        sqlx::query("UPDATE users SET avatar_url = $1 WHERE public_id = $2")
            .bind(illegal)
            .bind(Uuid::parse_str(&public_id).expect("publicId 非法"))
            .execute(&env.pool)
            .await
            .expect("更新 avatar_url 失败");
        let response = send(
            &env.router,
            Call::new(Method::GET, &format!("/api/users/{public_id}/avatar")),
        )
        .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{illegal}");
        assert!(response.body.is_empty());
        let me = send(
            &env.router,
            Call::new(Method::GET, "/api/me/avatar").cookie(cookie.clone()),
        )
        .await;
        assert_eq!(me.status, StatusCode::NOT_FOUND, "{illegal}");
    }

    // 非法 publicId 与不存在的用户都 404。
    for path in [
        "/api/users/not-a-uuid/avatar",
        &format!("/api/users/{}/avatar", Uuid::new_v4()),
    ] {
        let response = send(&env.router, Call::new(Method::GET, path)).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    }

    // 已删除用户：匿名按 404；会话失效为 401。
    sqlx::query("UPDATE users SET deleted = true WHERE public_id = $1")
        .bind(Uuid::parse_str(&public_id).expect("publicId 非法"))
        .execute(&env.pool)
        .await
        .expect("软删除用户失败");
    let deleted = send(
        &env.router,
        Call::new(Method::GET, &format!("/api/users/{public_id}/avatar")),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NOT_FOUND);
    let session = send(
        &env.router,
        Call::new(Method::GET, "/api/me/avatar").cookie(cookie),
    )
    .await;
    assert_eq!(session.status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 头像上传的 multipart 边界与 413
// ---------------------------------------------------------------------------

#[tokio::test]
async fn avatar_upload_multipart_validation_and_limits() {
    let env = TestEnv::new().await;
    let (cookie, _) = register(&env, "avatar-limits").await;
    let boundary = "lycoris-limit-boundary";
    let png = rgba_png(3, 3);

    // 未登录：401（先认证，后读取 multipart）。
    let body = multipart_body(boundary, &[file_part("file", "x.png", "image/png", &png)]);
    let unauth = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body),
    )
    .await;
    assert_eq!(unauth.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauth.json()["message"], "Spring Security Error");

    // 非法 Origin：写来源校验先拒绝（403 中文文本）。
    let body = multipart_body(boundary, &[file_part("file", "x.png", "image/png", &png)]);
    let cross = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body)
            .cookie(cookie.clone())
            .header("origin", "https://evil.example.com"),
    )
    .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    assert!(
        cross
            .header(header::CONTENT_TYPE)
            .unwrap()
            .starts_with("text/plain")
    );

    // 缺 file：400。
    let body = multipart_body(boundary, &[field_part("metadata", b"{}")]);
    let missing = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.json()["code"], 400);

    // 空文件：400「文件为空」。
    let body = multipart_body(
        boundary,
        &[file_part("file", "empty.png", "image/png", b"")],
    );
    let empty = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert_eq!(empty.json()["code"], 400);
    assert_eq!(empty.json()["message"], "文件为空");

    // 重复 file：400。
    let body = multipart_body(
        boundary,
        &[
            file_part("file", "a.png", "image/png", &png),
            file_part("file", "b.png", "image/png", &png),
        ],
    );
    let duplicate = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);
    assert_eq!(duplicate.json()["code"], 400);

    // 无效图片：400（消息来自媒体核心）。
    let body = multipart_body(
        boundary,
        &[file_part("file", "bad.png", "image/png", b"not an image")],
    );
    let invalid = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.json()["code"], 400);

    // 超过 5 MiB：413，形状为 GlobalExceptionHandler。
    let big = vec![0u8; 5 * 1024 * 1024 + 1];
    let body = multipart_body(boundary, &[file_part("file", "big.png", "image/png", &big)]);
    let too_big = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(too_big.json()["code"], 413);
    assert_eq!(
        too_big.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片"
    );
    assert!(too_big.json()["data"].is_null());

    // 整个请求超过 8 MiB（合法小文件 + 尾随未知字段）：仍是 413 JSON，不是空体。
    let padding = vec![b'x'; 8 * 1024 * 1024];
    let body = multipart_body(
        boundary,
        &[
            file_part("file", "ok.png", "image/png", &png),
            field_part("trailing", &padding),
        ],
    );
    let over_total = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie.clone()),
    )
    .await;
    assert_eq!(over_total.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(over_total.json()["code"], 413);
    assert_eq!(
        over_total.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片"
    );

    // 已知 Content-Length 超过 8 MiB：外层请求上限提前拒绝，不读入请求体，仍是 413 JSON。
    let declared = (8 * 1024 * 1024 + 1).to_string();
    let body = multipart_body(boundary, &[file_part("file", "ok.png", "image/png", &png)]);
    let early = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body)
            .cookie(cookie.clone())
            .header("content-length", &declared),
    )
    .await;
    assert_eq!(early.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(early.json()["code"], 413);
    assert_eq!(
        early.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片"
    );

    // 总请求 >2 MiB（Axum 默认）但 <5 MiB：证明默认 2 MiB 已被 8 MiB 覆盖。
    let metadata = vec![b'm'; 3 * 1024 * 1024];
    let body = multipart_body(
        boundary,
        &[
            file_part("file", "ok.png", "image/png", &png),
            field_part("metadata", &metadata),
        ],
    );
    let passed = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body).cookie(cookie),
    )
    .await;
    assert_eq!(
        passed.status,
        StatusCode::OK,
        ">2MiB 且 <5MiB 的 multipart 必须通过，证明默认 2MiB 已覆盖"
    );
    assert_eq!(passed.json()["code"], 0);
}

// ---------------------------------------------------------------------------
// 受控 /uploads/markers 授权
// ---------------------------------------------------------------------------

#[tokio::test]
async fn marker_upload_reads_authorize_by_viewer() {
    let env = TestEnv::new().await;
    let (owner_cookie, owner_public_id) = register(&env, "upload-owner").await;
    let (other_cookie, _) = register(&env, "upload-other").await;
    let admin_cookie = insert_admin(&env, "upload-admin").await;

    write_marker_file(&env, "public.png", b"public-image");
    write_marker_file(&env, "private.png", b"private-image");

    insert_marker(
        &env,
        "公开图",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/public.png"),
    )
    .await;
    insert_marker(
        &env,
        "私有图",
        false,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/private.png"),
    )
    .await;

    // 公开图片匿名可读。
    let anon_public = send(
        &env.router,
        Call::new(Method::GET, "/uploads/markers/public.png"),
    )
    .await;
    assert_eq!(anon_public.status, StatusCode::OK);
    assert_eq!(anon_public.header(header::CACHE_CONTROL), Some("no-store"));
    assert_eq!(anon_public.body, b"public-image");

    // 私有图片：匿名/他人 404，属主/管理员 200。
    for (label, cookie) in [("anon", None), ("other", Some(&other_cookie))] {
        let call = Call::new(Method::GET, "/uploads/markers/private.png");
        let response = match cookie {
            Some(cookie) => send(&env.router, call.cookie(cookie.clone())).await,
            None => send(&env.router, call).await,
        };
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{label} 不应可见");
        assert!(response.body.is_empty());
    }
    for (label, cookie) in [("owner", &owner_cookie), ("admin", &admin_cookie)] {
        let response = send(
            &env.router,
            Call::new(Method::GET, "/uploads/markers/private.png").cookie(cookie.clone()),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "{label} 应可见");
        assert_eq!(response.body, b"private-image");
    }

    // 非法目录/文件/缺失 404。
    for path in [
        "/uploads/etc/a.png",
        "/uploads/markers/no-such-file.png",
        "/uploads/markers/bad%.png",
    ] {
        let response = send(&env.router, Call::new(Method::GET, path)).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn avatar_and_anonymous_marker_reads_do_not_need_redis() {
    let env = TestEnv::new().await;
    let (_, owner_public_id) = register(&env, "upload-noredis").await;
    write_marker_file(&env, "public.png", b"public-image");
    insert_marker(
        &env,
        "公开图",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/public.png"),
    )
    .await;
    let avatar_dir = env.upload_root().join("avatars");
    std::fs::create_dir_all(&avatar_dir).unwrap();
    std::fs::write(avatar_dir.join("anon.png"), b"avatar-image").unwrap();

    // Redis 不可用：avatars 与匿名 markers 读取都不应加载会话。
    let router = env.router_with_redis(unreachable_redis());

    let avatar = send(&router, Call::new(Method::GET, "/uploads/avatars/anon.png")).await;
    assert_eq!(avatar.status, StatusCode::OK, "匿名头像读取不应依赖 Redis");
    assert_eq!(avatar.body, b"avatar-image");

    let marker = send(
        &router,
        Call::new(Method::GET, "/uploads/markers/public.png"),
    )
    .await;
    assert_eq!(
        marker.status,
        StatusCode::OK,
        "匿名公开点位图片不应依赖 Redis"
    );
    assert_eq!(marker.body, b"public-image");
}

// ---------------------------------------------------------------------------
// 全局请求体上限与 CORS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cors_headers_present_on_request_limit_413() {
    let env = TestEnv::new().await;
    let (cookie, _) = register(&env, "cors-user").await;
    let declared = (8 * 1024 * 1024 + 1).to_string();

    // 已知 Content-Length 超限（不读 body 的 GET /health/live）：413 且带 CORS 头与 Vary。
    let response = send(
        &env.router,
        Call::new(Method::GET, "/health/live")
            .header("origin", ALLOWED_ORIGIN)
            .header("content-length", &declared),
    )
    .await;
    assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.json()["code"], 413);
    assert_eq!(
        response.header(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(ALLOWED_ORIGIN),
        "已允许 Origin 的 413 必须带 Access-Control-Allow-Origin"
    );
    assert_eq!(
        response.header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
        Some("true")
    );
    let vary = response
        .header(header::VARY)
        .unwrap_or_default()
        .to_lowercase();
    assert!(vary.contains("origin"), "413 必须保留 CORS 的 Vary: {vary}");

    // 非白名单 Origin：413 但不发 allow-origin。
    let denied = send(
        &env.router,
        Call::new(Method::GET, "/health/live")
            .header("origin", "https://evil.example.com")
            .header("content-length", &declared),
    )
    .await;
    assert_eq!(denied.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        denied.header(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
        "非白名单 Origin 不得收到 allow-origin"
    );

    // 无 Content-Length 的流式 multipart 超限：同样 413 且带 CORS 头（写来源也允许）。
    let boundary = "lycoris-cors-boundary";
    let png = rgba_png(3, 3);
    let padding = vec![b'x'; 8 * 1024 * 1024];
    let body = multipart_body(
        boundary,
        &[
            file_part("file", "ok.png", "image/png", &png),
            field_part("trailing", &padding),
        ],
    );
    let streamed = send(
        &env.router,
        Call::multipart("/api/me/avatar", boundary, body)
            .cookie(cookie)
            .header("origin", ALLOWED_ORIGIN),
    )
    .await;
    assert_eq!(streamed.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(streamed.json()["code"], 413);
    assert_eq!(
        streamed.header(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(ALLOWED_ORIGIN),
        "流式 multipart 超限的 413 必须带 Access-Control-Allow-Origin"
    );
    assert_eq!(
        streamed.header(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
        Some("true")
    );
}

#[tokio::test]
async fn global_body_limit_covers_routes_that_do_not_read_body() {
    let env = TestEnv::new().await;

    // GET /health/live 不读 body，但 tower-http 全局限制仍按 Content-Length 提前 413。
    let declared = (8 * 1024 * 1024 + 1).to_string();
    let response = send(
        &env.router,
        Call::new(Method::GET, "/health/live").header("content-length", &declared),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "不读 body 的接口也必须受全局 8 MiB 上限约束"
    );
    assert_eq!(response.json()["code"], 413);
    assert_eq!(
        response.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片"
    );
}

#[tokio::test]
async fn json_body_length_limit_is_structured_and_read_failure_is_400() {
    let env = TestEnv::new().await;

    // 超过 JSON 提取器 64 KiB 上限：与本轮统一 413 契约同一文案（不再单独造字），
    // 且已允许 Origin 时 CORS 头仍在。
    let huge = serde_json::json!({ "username": "x".repeat(70 * 1024), "password": "y" });
    let oversized = send(
        &env.router,
        Call::json("/api/login", huge).header("origin", ALLOWED_ORIGIN),
    )
    .await;
    assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(oversized.json()["code"], 413);
    assert_eq!(
        oversized.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片",
        "JSON 请求体超限必须复用统一 413 文案"
    );
    assert_eq!(
        oversized.header(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(ALLOWED_ORIGIN),
        "JSON 提取器 413 也必须保留 CORS 头"
    );

    // 请求体读取中途出错（截断/网络）：400 请求体读取失败，不是 413。
    let failing = stream::once(async { Ok::<Bytes, std::io::Error>(Bytes::from_static(b"{")) })
        .chain(stream::once(async {
            Err::<Bytes, std::io::Error>(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated",
            ))
        }));
    let broken = send(
        &env.router,
        Call::raw(
            Method::POST,
            "/api/login",
            "application/json",
            Body::from_stream(failing),
        ),
    )
    .await;
    assert_eq!(broken.status, StatusCode::BAD_REQUEST);
    assert_eq!(broken.text(), "请求体读取失败");
}

// ---------------------------------------------------------------------------
// 请求 ID（最外层访问日志中间件）
// ---------------------------------------------------------------------------

/// 成功、404 fallback 与全局 body limit 413 都由服务端生成合法且互不相同的 `X-Request-ID`；
/// 客户端提供的 ID 不被照搬；允许来源可通过 expose-headers 在浏览器读取该头。
#[tokio::test]
async fn responses_carry_server_generated_request_id() {
    let env = TestEnv::new().await;
    let client_supplied = "11111111-1111-4111-8111-111111111111";
    let declared = (8 * 1024 * 1024 + 1).to_string();

    // 成功：同时携带客户端伪造的 X-Request-ID，服务端必须忽略并生成自己的。
    let ok = send(
        &env.router,
        Call::new(Method::GET, "/health/live")
            .header("origin", ALLOWED_ORIGIN)
            .header("x-request-id", client_supplied),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    let ok_id = response_request_id(&ok);

    // 404 fallback（无匹配路由模板）。
    let missing = send(&env.router, Call::new(Method::GET, "/no/such/route")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let missing_id = response_request_id(&missing);

    // 全局 body limit 413（已知 Content-Length 超限，不读 body）。
    let too_large = send(
        &env.router,
        Call::new(Method::GET, "/health/live").header("content-length", &declared),
    )
    .await;
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
    let too_large_id = response_request_id(&too_large);

    // 三个 ID 各自合法（已在 `response_request_id` 校验），且互不相同、均非客户端值。
    let forged = Uuid::parse_str(client_supplied).expect("测试用客户端 ID 应为合法 UUID");
    assert_ne!(ok_id, forged, "不得照搬客户端提供的 X-Request-ID");
    assert_ne!(ok_id, missing_id);
    assert_ne!(ok_id, too_large_id);
    assert_ne!(missing_id, too_large_id);

    // 允许来源：既有 allow-origin，又通过 expose-headers 暴露 X-Request-ID 供浏览器读取。
    assert_eq!(
        ok.header(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(ALLOWED_ORIGIN)
    );
    let expose = ok
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS)
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        expose.contains("x-request-id"),
        "允许来源必须能读取 X-Request-ID，实际 Access-Control-Expose-Headers: {expose}"
    );
}

fn response_request_id(response: &Resp) -> Uuid {
    let raw = response
        .header(HeaderName::from_static("x-request-id"))
        .expect("响应缺少 X-Request-ID");
    Uuid::parse_str(raw).expect("X-Request-ID 不是合法 UUID")
}

// ---------------------------------------------------------------------------
// 阶段 3：点位图片上传
// ---------------------------------------------------------------------------

#[tokio::test]
async fn marker_image_upload_creates_pending_and_localizes() {
    let env = TestEnv::new().await;
    let (owner_cookie, owner_public_id) = register(&env, "img-owner").await;
    let (proposer_cookie, _) = register(&env, "img-proposer").await;
    let (other_cookie, _) = register(&env, "img-other").await;
    let admin_cookie = login_admin(&env, "img-admin").await;

    let marker = insert_marker(
        &env,
        "原点位",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/original.png"),
    )
    .await;
    let hash = source_hash_components("zh", Some("原点位"), None);
    insert_translation(&env, marker, "en", "English Marker", &hash).await;

    // 匿名：认证先于解析 multipart，返回安全入口固定 401 JSON。
    let boundary = "lycoris-marker-img-boundary";
    let png = rgba_png(4, 3);
    let body = multipart_body(boundary, &[file_part("file", "m.png", "image/png", &png)]);
    let unauth = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{marker}/image"), boundary, body),
    )
    .await;
    assert_eq!(unauth.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauth.json()["message"], "Spring Security Error");

    // 属主上传：200，按 `lang=en` 本地化的原点位；只建 PENDING 提案，不换图。
    let body = multipart_body(boundary, &[file_part("file", "m.png", "image/png", &png)]);
    let uploaded = send(
        &env.router,
        Call::multipart(
            &format!("/api/markers/{marker}/image?lang=en"),
            boundary,
            body,
        )
        .cookie(owner_cookie.clone())
        .header("origin", ALLOWED_ORIGIN),
    )
    .await;
    assert_eq!(
        uploaded.status,
        StatusCode::OK,
        "上传应成功: {}",
        uploaded.text()
    );
    let json = uploaded.json();
    assert_eq!(json["id"], marker);
    assert_eq!(json["title"], "English Marker", "响应应按 lang 本地化");
    assert_eq!(json["contentLanguage"], "en");
    assert_eq!(
        json["markImage"], "/uploads/markers/original.png",
        "提交提案不得直接换图"
    );
    assert!(
        uploaded.header(header::VARY).is_some(),
        "本地化成功响应必须带 Vary"
    );

    let (status, username, public_id, image_url): (String, String, Option<String>, String) =
        sqlx::query_as(
            "SELECT status, proposer_username, proposer_public_id, image_url
             FROM marker_image_proposals WHERE marker_id = $1",
        )
        .bind(marker)
        .fetch_one(&env.pool)
        .await
        .expect("读取提案失败");
    assert_eq!(status, "PENDING");
    assert_eq!(username, "img-owner");
    assert_eq!(public_id.as_deref(), Some(owner_public_id.as_str()));
    assert!(
        image_url.starts_with(&format!("/uploads/markers/proposal-marker-{marker}-")),
        "实际 URL：{image_url}"
    );
    let (version, mark_image) = marker_row_state(&env, marker).await;
    assert_eq!(version, 0, "提交提案不得推进点位版本");
    assert_eq!(mark_image.as_deref(), Some("/uploads/markers/original.png"));

    // 待审图片：匿名/他人 404；属主/管理员可见。
    let filename = image_url.strip_prefix("/uploads/markers/").unwrap();
    let pending_path = format!("/uploads/markers/{filename}");
    let anon = send(&env.router, Call::new(Method::GET, &pending_path)).await;
    assert_eq!(anon.status, StatusCode::NOT_FOUND);
    let other = send(
        &env.router,
        Call::new(Method::GET, &pending_path).cookie(other_cookie),
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    for (label, cookie) in [("owner", &owner_cookie), ("admin", &admin_cookie)] {
        let visible = send(
            &env.router,
            Call::new(Method::GET, &pending_path).cookie(cookie.clone()),
        )
        .await;
        assert_eq!(visible.status, StatusCode::OK, "{label} 应可读待审图片");
    }

    // 非属主的提案作者：对公开点位提交后也可读自己的待审图片（提案作者 + 可见）。
    let body = multipart_body(boundary, &[file_part("file", "p.png", "image/png", &png)]);
    let proposer_upload = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{marker}/image"), boundary, body)
            .cookie(proposer_cookie.clone()),
    )
    .await;
    assert_eq!(proposer_upload.status, StatusCode::OK);
    let proposer_url: String = sqlx::query_scalar(
        "SELECT image_url FROM marker_image_proposals
         WHERE marker_id = $1 AND proposer_username = 'img-proposer'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(marker)
    .fetch_one(&env.pool)
    .await
    .expect("读取提案作者图片失败");
    let proposer_path = format!(
        "/uploads/markers/{}",
        proposer_url.strip_prefix("/uploads/markers/").unwrap()
    );
    let proposer_anon = send(&env.router, Call::new(Method::GET, &proposer_path)).await;
    assert_eq!(proposer_anon.status, StatusCode::NOT_FOUND);
    let proposer_read = send(
        &env.router,
        Call::new(Method::GET, &proposer_path).cookie(proposer_cookie),
    )
    .await;
    assert_eq!(
        proposer_read.status,
        StatusCode::OK,
        "提案作者对公开点位的待审图片应可读"
    );
}

#[tokio::test]
async fn marker_image_upload_validation_and_413_shape() {
    let env = TestEnv::new().await;
    let (owner_cookie, owner_public_id) = register(&env, "img-limit-owner").await;
    let (other_cookie, _) = register(&env, "img-limit-other").await;
    let private = insert_marker(&env, "私有", false, "APPROVED", &owner_public_id, None).await;
    let public = insert_marker(&env, "公开", true, "APPROVED", &owner_public_id, None).await;
    let boundary = "lycoris-marker-limit";
    let png = rgba_png(3, 3);

    // 写来源校验先于解析/保存：跨站写请求被中间件拒绝。
    let body = multipart_body(boundary, &[file_part("file", "x.png", "image/png", &png)]);
    let cross = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie.clone())
            .header("sec-fetch-site", "cross-site"),
    )
    .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);
    assert_eq!(cross.text(), "跨站请求被拒绝");

    // 不存在的点位：404 文本。
    let body = multipart_body(boundary, &[file_part("file", "x.png", "image/png", &png)]);
    let missing_marker = send(
        &env.router,
        Call::multipart("/api/markers/99999999/image", boundary, body).cookie(owner_cookie.clone()),
    )
    .await;
    assert_eq!(missing_marker.status, StatusCode::NOT_FOUND);
    assert_eq!(missing_marker.text(), "点位不存在");

    // 不可见点位：他人 404 文本，且不落盘。
    let files_before = std::fs::read_dir(env.upload_root().join("markers"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    let body = multipart_body(boundary, &[file_part("file", "x.png", "image/png", &png)]);
    let invisible = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{private}/image"), boundary, body)
            .cookie(other_cookie),
    )
    .await;
    assert_eq!(invisible.status, StatusCode::NOT_FOUND);
    assert_eq!(invisible.text(), "点位不存在");
    let files_after = std::fs::read_dir(env.upload_root().join("markers"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    assert_eq!(files_before, files_after, "不可见点位不得落盘新文件");

    // 缺 file：400 文本（复用头像已验证的 multipart helper）。
    let body = multipart_body(boundary, &[field_part("metadata", b"{}")]);
    let missing = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie.clone()),
    )
    .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.text(), "缺少文件");

    // 空文件：400「文件为空」文本。
    let body = multipart_body(
        boundary,
        &[file_part("file", "empty.png", "image/png", b"")],
    );
    let empty = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie.clone()),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert_eq!(empty.text(), "文件为空");

    // 非法图片：400 文本（消息来自媒体核心）。
    let body = multipart_body(
        boundary,
        &[file_part("file", "bad.png", "image/png", b"nope")],
    );
    let invalid = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie.clone()),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert!(!invalid.text().is_empty(), "非法图片应为中文纯文本 400");

    // 超过 5 MiB：413 且为统一 ApiResponse 形状。
    let big = vec![0u8; 5 * 1024 * 1024 + 1];
    let body = multipart_body(boundary, &[file_part("file", "big.png", "image/png", &big)]);
    let too_big = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie.clone()),
    )
    .await;
    assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(too_big.json()["code"], 413);
    assert_eq!(
        too_big.json()["message"],
        "上传文件过大，请选择 5MB 以内的图片"
    );
    assert!(too_big.json()["data"].is_null());

    // 整个请求超过 8 MiB（尾随未知字段）：仍是 413 ApiResponse，不是空体。
    let padding = vec![b'x'; 8 * 1024 * 1024];
    let body = multipart_body(
        boundary,
        &[
            file_part("file", "ok.png", "image/png", &png),
            field_part("trailing", &padding),
        ],
    );
    let over_total = send(
        &env.router,
        Call::multipart(&format!("/api/markers/{public}/image"), boundary, body)
            .cookie(owner_cookie),
    )
    .await;
    assert_eq!(over_total.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(over_total.json()["code"], 413);
}

// ---------------------------------------------------------------------------
// 阶段 3：管理员图片提案与清理
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_marker_image_routes_require_current_admin_role() {
    let env = TestEnv::new().await;
    let (user_cookie, _) = register(&env, "img-plain-user").await;

    // 匿名：安全入口固定 401 JSON。
    let anon = send(
        &env.router,
        Call::new(Method::GET, "/api/admin/markers/pending-images"),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anon.json()["message"], "Spring Security Error");

    // 普通用户：403 Spring Boot 默认 JSON（含 path/status/error）。
    let user = send(
        &env.router,
        Call::new(Method::GET, "/api/admin/markers/pending-images").cookie(user_cookie.clone()),
    )
    .await;
    assert_eq!(user.status, StatusCode::FORBIDDEN);
    let user_body = user.json();
    assert_eq!(user_body["status"], 403);
    assert_eq!(user_body["error"], "Forbidden");
    assert_eq!(user_body["path"], "/api/admin/markers/pending-images");
    let user_approve = send(
        &env.router,
        Call::new(Method::POST, "/api/admin/markers/image-proposals/1/approve").cookie(user_cookie),
    )
    .await;
    assert_eq!(user_approve.status, StatusCode::FORBIDDEN);
    assert_eq!(user_approve.json()["status"], 403);

    // Admins reach every handler without secondary verification; unknown proposals are 404.
    let unverified = insert_admin(&env, "img-unverified-admin").await;
    for path in [
        "/api/admin/markers/pending-images",
        "/api/admin/markers/image-proposals/1/approve",
        "/api/admin/markers/image-proposals/1/reject",
        "/api/admin/markers/cleanup-missing-images",
    ] {
        let method = if path.ends_with("pending-images") {
            Method::GET
        } else {
            Method::POST
        };
        let response = send(
            &env.router,
            Call::new(method, path).cookie(unverified.clone()),
        )
        .await;
        let expected = if path.contains("image-proposals") {
            StatusCode::NOT_FOUND
        } else if path.ends_with("cleanup-missing-images") {
            StatusCode::GONE
        } else {
            StatusCode::OK
        };
        assert_eq!(response.status, expected, "{path}");
    }

    // The pending list remains a plain JSON array.
    let verified = login_admin(&env, "img-verified-admin").await;
    let list = send(
        &env.router,
        Call::new(Method::GET, "/api/admin/markers/pending-images").cookie(verified),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.json().is_array());
}

#[tokio::test]
async fn admin_image_proposal_approve_returns_localized_marker_and_reports_errors() {
    let env = TestEnv::new().await;
    let (_owner_cookie, owner_public_id) = register(&env, "approve-owner").await;
    let admin = login_admin(&env, "approve-admin").await;

    let marker = insert_marker(&env, "待审图", true, "APPROVED", &owner_public_id, None).await;
    let hash = source_hash_components("zh", Some("待审图"), None);
    insert_translation(&env, marker, "en", "Approved Image", &hash).await;
    write_marker_file(&env, "pending-approve.png", b"pending");
    let proposal = insert_image_proposal(
        &env,
        marker,
        "待审图",
        "approve-owner",
        &owner_public_id,
        "/uploads/markers/pending-approve.png",
        "PENDING",
    )
    .await;

    // 待审清单：8 字段、createdAt DESC。
    let list = send(
        &env.router,
        Call::new(Method::GET, "/api/admin/markers/pending-images").cookie(admin.clone()),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    let item = &list.json()[0];
    for key in [
        "id",
        "markerId",
        "markerTitle",
        "proposerUsername",
        "proposerPublicId",
        "imageUrl",
        "status",
        "createdAt",
    ] {
        assert!(item.get(key).is_some(), "待审清单缺少字段 {key}");
    }
    assert_eq!(item["id"], proposal);

    // 批准：200 本地化 MarkerDto，markImage 换为提案图、version 推进。
    let approved = send(
        &env.router,
        Call::new(
            Method::POST,
            &format!("/api/admin/markers/image-proposals/{proposal}/approve?lang=en"),
        )
        .cookie(admin.clone()),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text());
    assert_eq!(approved.json()["id"], marker);
    assert_eq!(approved.json()["title"], "Approved Image");
    assert_eq!(
        approved.json()["markImage"],
        "/uploads/markers/pending-approve.png"
    );
    assert_eq!(approved.json()["version"], 1);
    assert!(approved.header(header::VARY).is_some());
    let (status, reviewer) = proposal_row_state(&env, proposal).await;
    assert_eq!(status, "APPROVED");
    assert_eq!(reviewer.as_deref(), Some("approve-admin"));

    // 批准后公开点位图匿名可读。
    let anon = send(
        &env.router,
        Call::new(Method::GET, "/uploads/markers/pending-approve.png"),
    )
    .await;
    assert_eq!(anon.status, StatusCode::OK);

    // 重复处理：400 文本。
    let duplicate = send(
        &env.router,
        Call::new(
            Method::POST,
            &format!("/api/admin/markers/image-proposals/{proposal}/approve"),
        )
        .cookie(admin.clone()),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);
    assert_eq!(duplicate.text(), "该提案已处理");

    // 不存在提案：404 文本。
    let missing = send(
        &env.router,
        Call::new(
            Method::POST,
            "/api/admin/markers/image-proposals/99999999/approve",
        )
        .cookie(admin.clone()),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.text(), "图片提案不存在");

    // 关联点位不存在：404「关联点位不存在」。
    let orphan = insert_image_proposal(
        &env,
        9_999_999,
        "孤儿",
        "approve-owner",
        &owner_public_id,
        "/uploads/markers/orphan.png",
        "PENDING",
    )
    .await;
    let orphan_approve = send(
        &env.router,
        Call::new(
            Method::POST,
            &format!("/api/admin/markers/image-proposals/{orphan}/approve"),
        )
        .cookie(admin),
    )
    .await;
    assert_eq!(orphan_approve.status, StatusCode::NOT_FOUND);
    assert_eq!(orphan_approve.text(), "关联点位不存在");
}

#[tokio::test]
async fn rejected_proposal_and_deleted_marker_keep_media_core_permissions() {
    let env = TestEnv::new().await;
    let (proposer_cookie, proposer_public_id) = register(&env, "reject-proposer").await;
    let admin = login_admin(&env, "reject-admin").await;
    let owner_public_id = Uuid::new_v4().to_string();
    let marker = insert_marker(&env, "公开拒绝", true, "APPROVED", &owner_public_id, None).await;
    write_marker_file(&env, "reject.png", b"reject");
    let proposal = insert_image_proposal(
        &env,
        marker,
        "公开拒绝",
        "reject-proposer",
        &proposer_public_id,
        "/uploads/markers/reject.png",
        "PENDING",
    )
    .await;

    // 拒绝：200 空体，记录审核人。
    let rejected = send(
        &env.router,
        Call::new(
            Method::POST,
            &format!("/api/admin/markers/image-proposals/{proposal}/reject"),
        )
        .cookie(admin.clone()),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::OK);
    assert!(rejected.body.is_empty());
    let (status, reviewer) = proposal_row_state(&env, proposal).await;
    assert_eq!(status, "REJECTED");
    assert_eq!(reviewer.as_deref(), Some("reject-admin"));

    // 提案状态不参与图片权限：提案作者仍可读，匿名不可读。
    let anon = send(
        &env.router,
        Call::new(Method::GET, "/uploads/markers/reject.png"),
    )
    .await;
    assert_eq!(anon.status, StatusCode::NOT_FOUND);
    let proposer = send(
        &env.router,
        Call::new(Method::GET, "/uploads/markers/reject.png").cookie(proposer_cookie.clone()),
    )
    .await;
    assert_eq!(proposer.status, StatusCode::OK);

    // 重复拒绝：400 文本。
    let again = send(
        &env.router,
        Call::new(
            Method::POST,
            &format!("/api/admin/markers/image-proposals/{proposal}/reject"),
        )
        .cookie(admin.clone()),
    )
    .await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    assert_eq!(again.text(), "该提案已处理");

    // 关联点位删除后：历史提案不授权任何人（含提案作者与管理员）。
    sqlx::query("DELETE FROM map_markers WHERE id = $1")
        .bind(marker)
        .execute(&env.pool)
        .await
        .expect("删除点位失败");
    for (label, cookie) in [("proposer", &proposer_cookie), ("admin", &admin)] {
        let response = send(
            &env.router,
            Call::new(Method::GET, "/uploads/markers/reject.png").cookie(cookie.clone()),
        )
        .await;
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{label} 不得读取已删点位的历史提案图片"
        );
    }
}

#[tokio::test]
async fn concurrent_admin_approval_over_http_only_one_succeeds() {
    let env = TestEnv::new().await;
    let (_owner_cookie, owner_public_id) = register(&env, "race-owner").await;
    let admin_a = login_admin(&env, "race-admin-a").await;
    let admin_b = login_admin(&env, "race-admin-b").await;
    let marker = insert_marker(&env, "并发审批", true, "APPROVED", &owner_public_id, None).await;
    write_marker_file(&env, "race.png", b"race");
    let proposal = insert_image_proposal(
        &env,
        marker,
        "并发审批",
        "race-owner",
        &owner_public_id,
        "/uploads/markers/race.png",
        "PENDING",
    )
    .await;

    let uri = format!("/api/admin/markers/image-proposals/{proposal}/approve");
    let (first, second) = tokio::join!(
        send(&env.router, Call::new(Method::POST, &uri).cookie(admin_a)),
        send(&env.router, Call::new(Method::POST, &uri).cookie(admin_b)),
    );
    let statuses = [first.status, second.status];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "并发审批只能有一个成功：{statuses:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|s| **s == StatusCode::BAD_REQUEST)
            .count(),
        1,
        "另一个应为 400 已处理：{statuses:?}"
    );
    let (status, reviewer) = proposal_row_state(&env, proposal).await;
    assert_eq!(status, "APPROVED");
    assert!(
        reviewer.as_deref() == Some("race-admin-a") || reviewer.as_deref() == Some("race-admin-b")
    );
    assert_eq!(marker_row_state(&env, marker).await.0, 1);
}

#[tokio::test]
async fn retired_cleanup_endpoint_preserves_every_reference_and_file() {
    let env = TestEnv::new().await;
    let admin = login_admin(&env, "cleanup-admin").await;
    let owner_public_id = Uuid::new_v4().to_string();
    write_marker_file(&env, "present.png", b"present");

    let present = insert_marker(
        &env,
        "存在",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/present.png"),
    )
    .await;
    let missing = insert_marker(
        &env,
        "缺失",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/markers/missing.png"),
    )
    .await;
    // 非 `/uploads/markers/` 前缀不计入检查。
    insert_marker(
        &env,
        "头像前缀",
        true,
        "APPROVED",
        &owner_public_id,
        Some("/uploads/avatars/x.png"),
    )
    .await;

    let response = send(
        &env.router,
        Call::new(Method::POST, "/api/admin/markers/cleanup-missing-images").cookie(admin),
    )
    .await;
    assert_eq!(response.status, StatusCode::GONE);
    assert_eq!(response.text(), "批量清理功能已停用");

    assert_eq!(
        marker_row_state(&env, present).await.1.as_deref(),
        Some("/uploads/markers/present.png"),
        "存在的普通文件必须保留引用"
    );
    assert_eq!(
        marker_row_state(&env, missing).await.1.as_deref(),
        Some("/uploads/markers/missing.png")
    );
    assert!(
        env.upload_root()
            .join("markers")
            .join("present.png")
            .is_file(),
        "清理不得删除文件"
    );
}

// S5 resume tests use real PostgreSQL rows and a fresh router between chunks.
fn resume_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn resume_start(id: i64, token: &str, request: &str, bytes: &[u8]) -> Call {
    Call::json(
        &format!("/api/markers/{id}/image-uploads"),
        serde_json::json!({
            "clientRequestId": request, "totalBytes": bytes.len(), "sha256": resume_digest(bytes)
        }),
    )
    .cookie(token)
    .header("Origin", ALLOWED_ORIGIN)
}
fn resume_chunk(url: &str, token: &str, offset: usize, bytes: &[u8]) -> Call {
    Call::raw(
        Method::POST,
        &format!("{url}/chunks/{offset}"),
        "application/octet-stream",
        Body::from(bytes.to_vec()),
    )
    .cookie(token)
    .header("Origin", ALLOWED_ORIGIN)
}
fn resume_complete(url: &str, token: &str) -> Call {
    Call::new(Method::POST, &format!("{url}/complete"))
        .cookie(token)
        .header("Origin", ALLOWED_ORIGIN)
}

#[tokio::test]
async fn photo_resume_survives_router_restart_and_completes_exactly_once() {
    let env = TestEnv::new().await;
    let (token, owner) = register(&env, "resume-owner").await;
    let id = insert_marker(&env, "Resume place", true, "PENDING", &owner, None).await;
    // Deterministic noise makes a valid multi-chunk PNG without external fixtures.
    let mut random = 17u32;
    let pixels: Vec<u8> = (0..400 * 400 * 4)
        .map(|_| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random as u8
        })
        .collect();
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, 400, 400, ExtendedColorType::Rgba8)
        .unwrap();
    assert!(bytes.len() > 262144);
    let request = Uuid::new_v4().to_string();
    let first = send(&env.router, resume_start(id, &token, &request, &bytes)).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.text());
    assert_eq!(first.header(header::CACHE_CONTROL), Some("no-store"));
    let receipt = first.json();
    let url = format!(
        "/api/markers/{id}/image-uploads/{}",
        receipt["uploadId"].as_str().unwrap()
    );
    let same = send(&env.router, resume_start(id, &token, &request, &bytes)).await;
    assert_eq!(same.json(), receipt);
    let first_chunk = send(&env.router, resume_chunk(&url, &token, 0, &bytes[..262144])).await;
    assert_eq!(first_chunk.json()["receivedBytes"], 262144);
    // Simulate a committed chunk whose response was lost, followed by process restart.
    let restarted = env.router_with_redis(connect_redis().await);
    let status = send(&restarted, Call::new(Method::GET, &url).cookie(&token)).await;
    assert_eq!(status.json()["receivedBytes"], 262144);
    let replay = send(&restarted, resume_chunk(&url, &token, 0, &bytes[..262144])).await;
    assert_eq!(replay.json(), first_chunk.json());
    let conflict = send(&restarted, resume_chunk(&url, &token, 0, &vec![0; 262144])).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    let missing = send(&restarted, resume_complete(&url, &token)).await;
    assert_eq!(missing.status, StatusCode::CONFLICT);
    for offset in (262144..bytes.len()).step_by(262144) {
        let result = send(
            &restarted,
            resume_chunk(
                &url,
                &token,
                offset,
                &bytes[offset..(offset + 262144).min(bytes.len())],
            ),
        )
        .await;
        assert_eq!(result.status, StatusCode::OK, "{}", result.text());
    }
    let (a, b) = tokio::join!(
        send(&restarted, resume_complete(&url, &token)),
        send(&restarted, resume_complete(&url, &token))
    );
    assert_eq!(a.status, StatusCode::OK, "{}", a.text());
    assert_eq!(b.json(), a.json());
    assert_eq!(a.json()["status"], "COMPLETED");
    assert_eq!(
        send(&restarted, resume_start(id, &token, &request, &bytes))
            .await
            .json(),
        a.json()
    );
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM marker_image_proposals WHERE marker_id=$1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let (size,): (i32,) = sqlx::query_as(
        "SELECT octet_length(staged_bytes) FROM marker_image_uploads WHERE marker_id=$1",
    )
    .bind(id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(size, 0);
    assert_eq!(marker_row_state(&env, id).await.1, None);
}

#[tokio::test]
async fn photo_resume_enforces_owner_origin_bytes_and_expiry() {
    let env = TestEnv::new().await;
    let (token, owner) = register(&env, "resume-a").await;
    let (other, _) = register(&env, "resume-b").await;
    let id = insert_marker(&env, "Public place", true, "APPROVED", &owner, None).await;
    let bytes = rgba_png(8, 8);
    let request = Uuid::new_v4().to_string();
    let response = send(&env.router, resume_start(id, &token, &request, &bytes)).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let upload = response.json()["uploadId"].as_str().unwrap().to_string();
    let url = format!("/api/markers/{id}/image-uploads/{upload}");
    assert_eq!(
        send(&env.router, Call::new(Method::GET, &url).cookie(&other))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&env.router, resume_chunk(&url, &other, 0, &bytes))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&env.router, resume_complete(&url, &other))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &env.router,
            resume_chunk(&url, &token, 0, &bytes).header("Origin", "https://untrusted.example")
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&env.router, resume_start(id, &token, &request, &[1, 2, 3]))
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&env.router, resume_chunk(&url, &token, 0, &vec![0; 262145]))
            .await
            .status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let broken = Body::from_stream(stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(std::io::Error::other("synthetic disconnect")),
    ]));
    let interrupted = send(
        &env.router,
        Call::raw(
            Method::POST,
            &format!("{url}/chunks/0"),
            "application/octet-stream",
            broken,
        )
        .cookie(&token)
        .header("Origin", ALLOWED_ORIGIN),
    )
    .await;
    assert_eq!(interrupted.status, StatusCode::REQUEST_TIMEOUT);
    assert_eq!(
        send(&env.router, Call::new(Method::GET, &url).cookie(&token))
            .await
            .json()["receivedBytes"],
        0
    );
    assert_eq!(
        send(
            &env.router,
            resume_chunk(&url, &token, 0, &vec![0; bytes.len()])
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        send(&env.router, resume_complete(&url, &token))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    sqlx::query(
        "UPDATE marker_image_uploads SET expires_at=now()-interval '1 second' WHERE upload_id=$1",
    )
    .bind(Uuid::parse_str(&upload).unwrap())
    .execute(&env.pool)
    .await
    .unwrap();
    assert_eq!(
        send(&env.router, Call::new(Method::GET, &url).cookie(&token))
            .await
            .status,
        StatusCode::GONE
    );
    assert_eq!(
        send(&env.router, resume_start(id, &token, &request, &bytes))
            .await
            .status,
        StatusCode::GONE
    );
    let (status, size): (String, i32) = sqlx::query_as(
        "SELECT status,octet_length(staged_bytes) FROM marker_image_uploads WHERE upload_id=$1",
    )
    .bind(Uuid::parse_str(&upload).unwrap())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!((status, size), ("EXPIRED".into(), 0));
    let private = insert_marker(&env, "Private", false, "PENDING", &owner, None).await;
    assert_eq!(
        send(
            &env.router,
            resume_start(private, &other, &Uuid::new_v4().to_string(), &bytes)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &env.router,
            resume_start(private, &token, &Uuid::new_v4().to_string(), &bytes)
        )
        .await
        .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn photo_resume_cleans_known_failed_completion_and_preserves_retry_bytes() {
    let env = TestEnv::new().await;
    let (token, owner) = register(&env, "resume-failure").await;
    let id = insert_marker(&env, "Resume failure", true, "PENDING", &owner, None).await;
    let bytes = rgba_png(8, 8);
    let start = send(
        &env.router,
        resume_start(id, &token, &Uuid::new_v4().to_string(), &bytes),
    )
    .await;
    assert_eq!(start.status, StatusCode::OK);
    let url = format!(
        "/api/markers/{id}/image-uploads/{}",
        start.json()["uploadId"].as_str().unwrap()
    );
    assert_eq!(
        send(&env.router, resume_chunk(&url, &token, 0, &bytes))
            .await
            .status,
        StatusCode::OK
    );
    sqlx::raw_sql("CREATE FUNCTION fail_s5_image() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic failure'; END $$; CREATE TRIGGER fail_s5 BEFORE INSERT ON marker_image_proposals FOR EACH ROW EXECUTE FUNCTION fail_s5_image();").execute(&env.pool).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            send(&env.router, resume_complete(&url, &token))
                .await
                .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            std::fs::read_dir(env.upload_root().join("markers"))
                .unwrap()
                .count(),
            0
        );
    }
    assert_eq!(
        send(&env.router, Call::new(Method::GET, &url).cookie(&token))
            .await
            .json()["receivedBytes"],
        bytes.len()
    );
    sqlx::query("DROP TRIGGER fail_s5 ON marker_image_proposals")
        .execute(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        send(&env.router, resume_complete(&url, &token))
            .await
            .json()["status"],
        "COMPLETED"
    );
    assert_eq!(
        std::fs::read_dir(env.upload_root().join("markers"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn photo_resume_completion_finishes_once_after_http_waiter_is_cancelled() {
    let env = TestEnv::new().await;
    let (token, owner) = register(&env, "resume-cancel").await;
    let id = insert_marker(&env, "Cancelled waiter", true, "PENDING", &owner, None).await;
    let bytes = rgba_png(8, 8);
    let start = send(
        &env.router,
        resume_start(id, &token, &Uuid::new_v4().to_string(), &bytes),
    )
    .await;
    let url = format!(
        "/api/markers/{id}/image-uploads/{}",
        start.json()["uploadId"].as_str().unwrap()
    );
    assert_eq!(
        send(&env.router, resume_chunk(&url, &token, 0, &bytes))
            .await
            .status,
        StatusCode::OK
    );
    let mut held = env.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM map_markers WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut *held)
        .await
        .unwrap();
    let router = env.router.clone();
    let request = resume_complete(&url, &token);
    let waiter = tokio::spawn(async move { send(&router, request).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_dir(env.upload_root().join("markers"))
                .is_ok_and(|items| items.count() == 1)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Same cancellation boundary as TimeoutLayer dropping the handler future.
    waiter.abort();
    let _ = waiter.await;
    held.rollback().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        loop {
            let (count,): (i64,)=sqlx::query_as("SELECT count(*) FROM marker_image_uploads WHERE marker_id=$1 AND status='COMPLETED'").bind(id).fetch_one(&env.pool).await.unwrap();
            if count==1 {break}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(
        send(&env.router, resume_complete(&url, &token))
            .await
            .json()["status"],
        "COMPLETED"
    );
    assert_eq!(
        std::fs::read_dir(env.upload_root().join("markers"))
            .unwrap()
            .count(),
        1
    );
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM marker_image_proposals WHERE marker_id=$1")
            .bind(id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn thumbnail_head_and_conditional_requests_always_recheck_permissions() {
    let env = TestEnv::new().await;
    let (owner_cookie, owner) = register(&env, "thumb-owner").await;
    write_marker_file(&env, "rendition.png", &rgba_png(800, 400));
    let marker = insert_marker(
        &env,
        "缩略图",
        true,
        "APPROVED",
        &owner,
        Some("/uploads/markers/rendition.png"),
    )
    .await;
    let url = "/uploads/markers/rendition.png?variant=thumb";
    let first = send(&env.router, Call::new(Method::GET, url)).await;
    assert_eq!(first.status, StatusCode::OK);
    let decoded = image::load_from_memory(&first.body).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (640, 320));
    let etag = first.header(header::ETAG).unwrap();
    let head = send(&env.router, Call::new(Method::HEAD, url)).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    assert_eq!(head.header(header::ETAG), Some(etag));
    assert_eq!(
        head.header(header::CONTENT_LENGTH).unwrap(),
        first.body.len().to_string()
    );
    assert!(head.headers.contains_key("x-lycoris-media-key"));
    let unchanged = send(
        &env.router,
        Call::new(Method::GET, url).header("If-None-Match", etag),
    )
    .await;
    assert_eq!(unchanged.status, StatusCode::NOT_MODIFIED);
    let mismatch = send(
        &env.router,
        Call::new(Method::GET, url).header("If-Match", "\"old\""),
    )
    .await;
    assert_eq!(mismatch.status, StatusCode::PRECONDITION_FAILED);
    sqlx::query("UPDATE map_markers SET is_public=false WHERE id=$1")
        .bind(marker)
        .execute(&env.pool)
        .await
        .unwrap();
    for method in [Method::GET, Method::HEAD] {
        let denied = send(
            &env.router,
            Call::new(method, url).header("If-None-Match", etag),
        )
        .await;
        assert_eq!(denied.status, StatusCode::NOT_FOUND);
        assert!(!denied.headers.contains_key("x-lycoris-media-key"));
    }
    assert_eq!(
        send(
            &env.router,
            Call::new(Method::GET, url).cookie(owner_cookie.clone())
        )
        .await
        .status,
        StatusCode::OK
    );
    sqlx::query("UPDATE map_markers SET deactivated=true WHERE id=$1")
        .bind(marker)
        .execute(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        send(
            &env.router,
            Call::new(Method::HEAD, url)
                .cookie(owner_cookie)
                .header("If-None-Match", etag)
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let invalid = send(
        &env.router,
        Call::new(Method::GET, "/uploads/markers/rendition.png?variant=99999"),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
}
