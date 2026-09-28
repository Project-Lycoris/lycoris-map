//! 阶段 2 认证与用户核心的真实 PG / Redis 集成测试。
//!
//! 每个用例创建 UUID 命名的临时库与随机 Redis 命名空间，不 `FLUSHALL`、不 `KEYS`，
//! 不触碰其它测试或生产数据。只连接回环地址上的合成测试服务。

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use fred::clients::Client;
use fred::interfaces::{ClientLike, HashesInterface, KeysInterface};
use fred::types::config::ServerConfig;
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::migrate;
use lycoris_backend::password::PasswordHasher;
use lycoris_backend::session::{NewSession, SessionStore, TransitionState};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};
use tower::ServiceExt;
use uuid::Uuid;

const DEFAULT_TEST_DATABASE_URL: &str =
    "postgres://lycoris:lycoris_local_test@127.0.0.1:55432/lycoris_rust";
const DEFAULT_TEST_REDIS_URL: &str = "redis://127.0.0.1:56379";
const UNREACHABLE_REDIS_URL: &str = "redis://127.0.0.1:1";
const COOKIE_NAME: &str = "LYCORIS_SESSION";
const ALLOWED_ORIGIN: &str = "https://app.example.com";

fn test_database_url() -> String {
    std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string())
}

fn test_redis_url() -> String {
    std::env::var("TEST_REDIS_URL").unwrap_or_else(|_| DEFAULT_TEST_REDIS_URL.to_string())
}

fn host_is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn assert_pg_loopback(options: &PgConnectOptions) {
    assert!(
        host_is_loopback(options.get_host()),
        "测试 PostgreSQL 必须位于回环地址"
    );
}

fn assert_redis_loopback(config: &fred::types::config::Config) {
    let host = match &config.server {
        ServerConfig::Centralized { server } => server.host.to_string(),
        ServerConfig::Clustered { hosts, .. } | ServerConfig::Sentinel { hosts, .. } => hosts
            .first()
            .map(|server| server.host.to_string())
            .unwrap_or_default(),
    };
    assert!(host_is_loopback(&host), "测试 Redis 必须位于回环地址");
}

struct TempDatabase {
    admin_options: PgConnectOptions,
    name: String,
    url: String,
}

impl TempDatabase {
    async fn create() -> Self {
        let admin_url = test_database_url();
        let admin_options =
            PgConnectOptions::from_str(&admin_url).expect("TEST_DATABASE_URL 格式非法");
        assert_pg_loopback(&admin_options);
        let name = format!("lycoris_test_{}", Uuid::new_v4().simple());

        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin_options.clone())
            .await
            .expect("无法连接测试 PostgreSQL，请先启动隔离测试服务");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin)
            .await
            .expect("创建临时测试库失败");
        admin.close().await;

        let url = admin_options
            .clone()
            .database(&name)
            .to_url_lossy()
            .to_string();
        Self {
            admin_options,
            name,
            url,
        }
    }

    fn url(&self) -> &str {
        &self.url
    }

    async fn connect_pool(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(5)
            .connect_with(self.admin_options.clone().database(&self.name))
            .await
            .expect("连接临时测试库失败")
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let admin_options = self.admin_options.clone();
        let name = self.name.clone();
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                eprintln!("[warn] 清理临时测试库 {name} 失败: 无法创建运行时");
                return;
            };
            runtime.block_on(async move {
                if let Ok(admin) = PgPoolOptions::new()
                    .max_connections(1)
                    .connect_with(admin_options)
                    .await
                {
                    // 仅删除本用例 UUID 命名的库，不按前缀枚举或批量操作。
                    let sql = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");
                    if let Err(error) = sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&admin).await
                    {
                        eprintln!("[warn] 清理临时测试库 {name} 失败: {error}");
                    }
                    admin.close().await;
                }
            });
        })
        .join();
    }
}

async fn connect_redis() -> Client {
    let url = test_redis_url();
    let config = fred::types::config::Config::from_url(&url).expect("测试 Redis URL 格式非法");
    assert_redis_loopback(&config);
    let client = fred::types::Builder::from_config(config)
        .build()
        .expect("构建测试 Redis 客户端失败");
    client
        .init()
        .await
        .expect("无法连接测试 Redis，请先启动隔离测试服务");
    client
}

fn unreachable_redis() -> Client {
    let config =
        fred::types::config::Config::from_url(UNREACHABLE_REDIS_URL).expect("测试 URL 格式非法");
    fred::types::Builder::from_config(config)
        .build()
        .expect("构建测试 Redis 客户端失败")
}

async fn connect_redis_url(url: &str) -> Option<Client> {
    let config = fred::types::config::Config::from_url(url).ok()?;
    let client = fred::types::Builder::from_config(config).build().ok()?;
    client.init().await.ok()?;
    Some(client)
}

/// 在测试 Redis 上创建受限 ACL 用户：允许所有命令但拒绝 DEL（用于 logout 删除失败回归）。
async fn acl_setuser_deny_del(admin: &Client, name: &str, password: &str) {
    let command =
        fred::types::CustomCommand::new_static("ACL", fred::types::ClusterHash::Random, false);
    let result = admin
        .custom::<fred::types::Value, _>(
            command,
            vec![
                fred::types::Value::from("SETUSER"),
                fred::types::Value::from(name),
                fred::types::Value::from("on"),
                fred::types::Value::from(format!(">{password}")),
                fred::types::Value::from("~*"),
                fred::types::Value::from("+@all"),
                fred::types::Value::from("-del"),
            ],
        )
        .await
        .expect("ACL SETUSER 失败");
    assert_eq!(result, fred::types::Value::from("OK"));
}

async fn acl_deluser(admin: &Client, name: &str) {
    let command =
        fred::types::CustomCommand::new_static("ACL", fred::types::ClusterHash::Random, false);
    let _ = admin
        .custom::<fred::types::Value, _>(
            command,
            vec![
                fred::types::Value::from("DELUSER"),
                fred::types::Value::from(name),
            ],
        )
        .await;
}

/// 测试环境：临时库 + Redis + Router + 独立命名空间。
struct TestEnv {
    _temp: TempDatabase,
    /// 临时上传根目录，保证测试不写真实 `uploads/` 且退出即清理。
    _upload: tempfile::TempDir,
    pool: PgPool,
    router: Router,
    /// 供测试直接调度改密切换窗口（服务公开方法），生产代码不含测试开关。
    state: AppState,
    config: Config,
}

impl TestEnv {
    async fn new() -> Self {
        Self::custom(|_| {}).await
    }

    async fn custom(configure: impl FnOnce(&mut Config)) -> Self {
        let temp = TempDatabase::create().await;
        let pool = temp.connect_pool().await;
        migrate::run(&pool).await.expect("执行基线迁移失败");
        let redis = connect_redis().await;

        let unique = Uuid::new_v4().simple().to_string();
        let mut config = Config::new(temp.url(), test_redis_url());
        config.session_namespace = format!("lycoris:test:{unique}:session");
        config.rate_limit_namespace = format!("lycoris:test:{unique}:ratelimit");
        // 测试统一使用 cost 4；生产默认 10。
        config.bcrypt_cost = 4;
        config.email_verification_secret = Some("test-email-secret-not-for-production-32".into());
        config.write_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
        configure(&mut config);
        let upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
        config.upload_dir = upload.path().to_path_buf();

        let state = AppState::new(pool.clone(), redis, config.clone()).expect("构造 AppState 失败");
        let router = build_router(state.clone());
        Self {
            _temp: temp,
            _upload: upload,
            pool,
            router,
            state,
            config,
        }
    }

    /// 直接构造使用坏 Redis 的 AppState（用于限流故障用例）。
    fn router_with_redis(&self, redis: Client) -> Router {
        build_router(
            AppState::new(self.pool.clone(), redis, self.config.clone())
                .expect("构造 AppState 失败"),
        )
    }

    async fn send(&self, request: TestRequest<'_>) -> Resp {
        let router = self.router.clone();
        send_with(&router, request).await
    }
}

struct TestRequest<'a> {
    method: Method,
    uri: &'a str,
    body: Option<serde_json::Value>,
    cookie: Option<String>,
    content_type: Option<&'a str>,
    headers: Vec<(String, String)>,
    connect_ip: IpAddr,
}

impl<'a> TestRequest<'a> {
    fn new(method: Method, uri: &'a str) -> Self {
        Self {
            method,
            uri,
            body: None,
            cookie: None,
            content_type: None,
            headers: Vec::new(),
            connect_ip: IpAddr::from([127, 0, 0, 1]),
        }
    }

    fn json(method: Method, uri: &'a str, body: serde_json::Value) -> Self {
        Self {
            body: Some(body),
            ..Self::new(method, uri)
        }
    }

    fn cookie(mut self, token: impl Into<String>) -> Self {
        self.cookie = Some(token.into());
        self
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn content_type(mut self, value: &'a str) -> Self {
        self.content_type = Some(value);
        self
    }

    fn connect_ip(mut self, ip: IpAddr) -> Self {
        self.connect_ip = ip;
        self
    }
}

async fn send_with(router: &Router, request: TestRequest<'_>) -> Resp {
    let mut builder = Request::builder().method(request.method).uri(request.uri);
    if let Some(content_type) = request.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    } else if request.body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let body = match request.body {
        Some(value) => Body::from(serde_json::to_vec(&value).unwrap()),
        None => Body::empty(),
    };
    let mut req = builder.body(body).unwrap();
    if let Some(token) = request.cookie {
        req.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE_NAME}={token}")).unwrap(),
        );
    }
    for (name, value) in request.headers {
        req.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(&value).unwrap(),
        );
    }
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(request.connect_ip, 51234)));
    let response = router.clone().oneshot(req).await.expect("调用路由失败");
    Resp::from(response).await
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    async fn from(response: Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("读取响应体失败");
        Self {
            status,
            headers,
            body: bytes.to_vec(),
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn session_cookie(&self) -> Option<String> {
        self.set_cookie(COOKIE_NAME)
    }

    fn set_cookie(&self, name: &str) -> Option<String> {
        for value in self.headers.get_all(header::SET_COOKIE) {
            let Some(raw) = value.to_str().ok() else {
                continue;
            };
            let first = raw.split(';').next().unwrap_or_default();
            if let Some(rest) = first.strip_prefix(&format!("{name}=")) {
                return Some(rest.to_string());
            }
        }
        None
    }
}

async fn register_body(env: &TestEnv, username: &str, email: &str) -> serde_json::Value {
    // Existing account/session tests use a synthetic delivered challenge.
    // The separate email-verification suite exercises send limits and failures.
    use lycoris_backend::email_verification::Purpose;
    let normalized = email.trim().to_lowercase();
    if let Ok(nonce) = env
        .state
        .email_codes
        .reserve(
            &normalized,
            Purpose::Register,
            "register",
            "127.0.0.1".parse().unwrap(),
            "123456",
        )
        .await
    {
        env.state
            .email_codes
            .finish(&normalized, Purpose::Register, &nonce, true)
            .await
            .unwrap();
    }
    serde_json::json!({
        "username": username,
        "nickname": username,
        "email": email,
        "password": "test-password",
        "website": "",
        "verificationCode": "123456",
    })
}

/// 注册并返回成功响应（含会话 Cookie）。
async fn register(env: &TestEnv, username: &str, email: &str) -> Resp {
    env.send(TestRequest::json(
        Method::POST,
        "/api/register",
        register_body(env, username, email).await,
    ))
    .await
}

/// 登录并返回响应。
async fn login(env: &TestEnv, username: &str, password: &str) -> Resp {
    env.send(TestRequest::json(
        Method::POST,
        "/api/login",
        serde_json::json!({ "username": username, "password": password }),
    ))
    .await
}

async fn insert_user(
    pool: &PgPool,
    username: &str,
    email: &str,
    password_hash: &str,
    role: &str,
) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO users (public_id, username, nickname, email, password, role, deleted, \
         session_version, row_version) VALUES ($1, $2, $2, $3, $4, $5, false, 0, 0) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(username)
    .bind(email)
    .bind(password_hash)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("插入测试用户失败")
}

/// Log in with an ordinary administrator account.
async fn login_admin(env: &TestEnv, username: &str, password: &str) -> String {
    let response = login(env, username, password).await;
    assert_eq!(response.status, StatusCode::OK);
    response.session_cookie().expect("admin login cookie")
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_login_me_and_cookie_stability() {
    let env = TestEnv::new().await;

    let created = register(&env, "alice", "alice@example.com").await;
    assert_eq!(created.status, StatusCode::OK);
    let data = created.json();
    assert_eq!(data["code"], 0);
    assert_eq!(data["data"]["username"], "alice");
    assert!(
        data["data"].get("role").is_none(),
        "UserResponse 不应包含 role"
    );
    assert!(data["data"].get("id").is_none(), "UserResponse 不应包含 id");
    let token = created.session_cookie().expect("注册应设置会话 Cookie");

    // 多次普通读取不应轮换标识，也不应重新下发 Cookie。
    for _ in 0..3 {
        let me = env
            .send(TestRequest::new(Method::GET, "/api/me").cookie(token.clone()))
            .await;
        assert_eq!(me.status, StatusCode::OK);
        assert_eq!(me.json()["data"]["publicId"], data["data"]["publicId"]);
        assert!(
            me.session_cookie().is_none(),
            "普通读取不得轮换或回写整份身份"
        );
    }

    // 用邮箱（大小写不敏感）也能登录。
    let by_email = login(&env, "ALICE@example.com", "test-password").await;
    assert_eq!(by_email.status, StatusCode::OK);

    // 失败登录不清除已有有效会话，也不下发 Cookie。
    let failed = login(&env, "alice", "wrong-password").await;
    assert_eq!(failed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(failed.json()["code"], 4001);
    assert!(failed.session_cookie().is_none());
    let still_valid = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token.clone()))
        .await;
    assert_eq!(still_valid.status, StatusCode::OK);
}

#[tokio::test]
async fn java_bcrypt_vectors_and_legacy_upgrade() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);

    // Public synthetic BCrypt vector generated with Spring Security Crypto 6.5.7.
    let vectors: &[(&str, &str)] = &[
        (
            "c3ludGhldGljLXBhc3N3b3Jk",
            "$2a$04$u2qdf8QkuekWvHmMdtdTaOFH9gC6mJZLEuDaxjRnJ9FFrBAe7jU6C",
        ),
        (
            "5ZCI5oiQ5a+G56CB8J+Mt2NhZsOp",
            "$2a$04$AJtrfbeEBSaAGrfKkpolOOqWXln8cJhsk2WYSwrpHf8NxMJ7OH3ju",
        ),
        (
            "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=",
            "$2a$04$srP7wITHEbKn35m3e2fpouq7vlH61yENUvI100zuCGN3MD2WMlKzm",
        ),
        (
            "YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFh",
            "$2a$04$NH3Jmh7ovHCmheJq89Zs9eHufbQA2lAD04VxDy90Eyu5xahoKTtSi",
        ),
        (
            "5aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW95aW9",
            "$2a$04$5hzKzShdnwBW2yAI8sDwreZr.gc71d9VMb.aOs9k2e2GD7T0B3JkW",
        ),
        (
            "YWJjAGRlZg==",
            "$2a$04$IUrV/EEV3YhSQCVF.m0iR..Z6X/qaTtMFq4Y7xW2sYYj737UkcoG6",
        ),
    ];
    for (index, (encoded, hash)) in vectors.iter().enumerate() {
        let password = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap();
        let username = format!("vector-{index}");
        insert_user(
            &env.pool,
            &username,
            &format!("{username}@example.com"),
            hash,
            "USER",
        )
        .await;
        let ok = login(&env, &username, &password).await;
        assert_eq!(ok.status, StatusCode::OK, "向量 {index} 应能登录");
        // 哈希字面量绝不作为明文密码。
        let literal = login(&env, &username, hash).await;
        assert_eq!(
            literal.status,
            StatusCode::UNAUTHORIZED,
            "向量 {index} 哈希字面量应被拒绝"
        );
    }

    // 存储中的哈希与 `{}` 前缀都不得回退明文。
    insert_user(
        &env.pool,
        "malformed",
        "malformed@example.com",
        "$2a$malformed",
        "USER",
    )
    .await;
    assert_eq!(
        login(&env, "malformed", "$2a$malformed").await.status,
        StatusCode::UNAUTHORIZED
    );
    insert_user(
        &env.pool,
        "delegated",
        "delegated@example.com",
        "{bcrypt}$2a$10$abcdefghijklmnopqrstuv",
        "USER",
    )
    .await;
    assert_eq!(
        login(&env, "delegated", "{bcrypt}$2a$10$abcdefghijklmnopqrstuv")
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    // 历史明文成功登录后升级为 BCrypt；旧明文不再有效。
    insert_user(
        &env.pool,
        "legacy",
        "legacy@example.com",
        "legacy-test-password",
        "USER",
    )
    .await;
    assert_eq!(
        login(&env, "legacy", "legacy-test-password").await.status,
        StatusCode::OK
    );
    let stored: String = sqlx::query_scalar("SELECT password FROM users WHERE username = $1")
        .bind("legacy")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("$2b$"), "明文应升级为 $2b$");
    assert!(
        hasher
            .verify("legacy-test-password".to_string(), stored.clone())
            .await
    );
    assert_eq!(
        login(&env, "legacy", "legacy-test-password").await.status,
        StatusCode::OK
    );
    assert_eq!(
        login(&env, "legacy", &stored).await.status,
        StatusCode::UNAUTHORIZED,
        "升级后的哈希字面量不能登录"
    );
}

#[tokio::test]
async fn duplicate_historical_accounts_are_not_authorized() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);

    let first = hasher.hash("password-one".to_string()).await.unwrap();
    let second = hasher.hash("password-two".to_string()).await.unwrap();

    // 用户名重复（含同密码与不同密码）。
    insert_user(&env.pool, "dup-user", "dup1@example.com", &first, "USER").await;
    insert_user(&env.pool, "dup-user", "dup2@example.com", &second, "USER").await;
    assert_eq!(
        login(&env, "dup-user", "password-one").await.status,
        StatusCode::UNAUTHORIZED
    );

    // 规范化邮箱重复。
    insert_user(&env.pool, "email-a", "DUP@example.com", &first, "USER").await;
    insert_user(&env.pool, "email-b", "dup@example.com", &second, "USER").await;
    assert_eq!(
        login(&env, "dup@example.com", "password-one").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn login_accepts_exact_usernames_and_case_insensitive_email_without_ambiguity() {
    let env = TestEnv::new().await;
    let password = "Case-Sensitive-Pass";
    let hash = PasswordHasher::new(4, 1)
        .hash(password.to_string())
        .await
        .unwrap();
    insert_user(
        &env.pool,
        "User@Handle",
        "Mixed.Email@Example.COM",
        &hash,
        "USER",
    )
    .await;
    for identity in ["User@Handle", " mixed.EMAIL@EXAMPLE.com "] {
        let response = login(&env, identity, password).await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.session_cookie().is_some());
    }
    for (identity, wrong_password) in [
        ("user@handle", password),
        ("mixed.email@example.com", "case-sensitive-pass"),
    ] {
        let response = login(&env, identity, wrong_password).await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert!(response.session_cookie().is_none());
    }

    // Both matches may refer to the same row; this remains one identity.
    insert_user(
        &env.pool,
        "same@example.com",
        "SAME@EXAMPLE.COM",
        &hash,
        "USER",
    )
    .await;
    assert_eq!(
        login(&env, "same@example.com", password).await.status,
        StatusCode::OK
    );

    // Never choose between two accounts or use the password to break a collision.
    insert_user(
        &env.pool,
        "collision@example.com",
        "separate@example.com",
        &hash,
        "USER",
    )
    .await;
    insert_user(
        &env.pool,
        "different-user",
        "COLLISION@EXAMPLE.COM",
        &hash,
        "USER",
    )
    .await;
    let response = login(&env, "collision@example.com", password).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(response.session_cookie().is_none());
}

#[tokio::test]
async fn concurrent_registration_creates_exactly_one_account() {
    let env = TestEnv::custom(|config| {
        config.register_rate_limit_max = 100;
    })
    .await;

    let body = register_body(&env, "race-user", "race@example.com").await;
    let mut handles = Vec::new();
    for _ in 0..10 {
        let router = env.router.clone();
        let body = body.clone();
        handles.push(tokio::spawn(async move {
            let request = TestRequest::json(Method::POST, "/api/register", body);
            send_with(&router, request).await
        }));
    }
    let mut ok = 0;
    let mut rejected = 0;
    for handle in handles {
        match handle.await.unwrap().status {
            StatusCode::OK => ok += 1,
            StatusCode::BAD_REQUEST => rejected += 1,
            other => panic!("意外状态: {other}"),
        }
    }
    assert_eq!(ok, 1, "并发注册只能有一个成功");
    assert_eq!(rejected, 9);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username = $1 OR lower(email) = $2")
            .bind("race-user")
            .bind("race@example.com")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "数据库不得出现重复账号");
}

#[tokio::test]
async fn logout_deletes_session_and_old_verify_cannot_revive() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    insert_user(&env.pool, "admin", "admin@example.com", &hash, "ADMIN").await;

    let token = login_admin(&env, "admin", "admin-pass").await;
    let logout = env
        .send(TestRequest::new(Method::POST, "/api/logout").cookie(token.clone()))
        .await;
    assert_eq!(logout.status, StatusCode::OK);
    // 清除 Cookie 必须以空值 + Max-Age=0 下发。
    let cleared = logout
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("Max-Age=0"));
    assert!(cleared, "退出应清除会话 Cookie");

    let me = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token.clone()))
        .await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    assert_eq!(me.json()["message"], "Spring Security Error");

    // 退出后旧会话的 verify CAS 失败，不会重建/复活会话状态。
    let revive = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/admin/verify",
                serde_json::json!({ "passcode": "second-pass" }),
            )
            .cookie(token),
        )
        .await;
    assert_eq!(revive.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn password_change_keeps_current_and_invalidates_others() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    insert_user(&env.pool, "admin", "admin@example.com", &hash, "ADMIN").await;

    let first = login(&env, "admin", "admin-pass").await;
    let token_a = first.session_cookie().unwrap();
    let second = login(&env, "admin", "admin-pass").await;
    let token_b = second.session_cookie().unwrap();
    assert_ne!(token_a, token_b);

    // Cached clients may still call the role-checked compatibility endpoint.
    let verify = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/admin/verify",
                serde_json::json!({ "passcode": "second-pass" }),
            )
            .cookie(token_a.clone()),
        )
        .await;
    assert_eq!(verify.status, StatusCode::OK);

    let changed = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/me/password",
                serde_json::json!({ "oldPassword": "admin-pass", "newPassword": "new-admin-pass" }),
            )
            .cookie(token_a.clone()),
        )
        .await;
    assert_eq!(changed.status, StatusCode::OK);
    assert_eq!(changed.json()["data"], serde_json::Value::Null);

    // 当前会话保留，其他会话失效。
    let current = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token_a.clone()))
        .await;
    assert_eq!(current.status, StatusCode::OK);
    let other = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token_b))
        .await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);

    // The current admin session keeps access after changing its password.
    let admin_list = env
        .send(TestRequest::new(Method::GET, "/api/admin/users").cookie(token_a))
        .await;
    assert_eq!(admin_list.status, StatusCode::OK);

    // 旧密码失效，新密码可登录。
    assert_eq!(
        login(&env, "admin", "admin-pass").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&env, "admin", "new-admin-pass").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_reset_delete_restore_invalidate_sessions() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let admin_hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    insert_user(
        &env.pool,
        "admin",
        "admin@example.com",
        &admin_hash,
        "ADMIN",
    )
    .await;
    let user_hash = hasher.hash("user-pass".to_string()).await.unwrap();
    let user_id = insert_user(&env.pool, "bob", "bob@example.com", &user_hash, "USER").await;

    let admin = login_admin(&env, "admin", "admin-pass").await;
    let user_token = login(&env, "bob", "user-pass")
        .await
        .session_cookie()
        .unwrap();

    // 重置密码使旧会话失效。
    let reset = env
        .send(
            TestRequest::new(
                Method::POST,
                &format!("/api/admin/users/{user_id}/reset-password"),
            )
            .cookie(admin.clone()),
        )
        .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert_eq!(reset.json()["message"], "密码已重置为默认密码");
    assert_eq!(
        env.send(TestRequest::new(Method::GET, "/api/me").cookie(user_token))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    // 默认密码可登录。
    assert_eq!(
        login(&env, "bob", "Lycoris123!").await.status,
        StatusCode::OK
    );

    // 删除用户；已删除不能再重置。
    let deleted = env
        .send(
            TestRequest::new(Method::DELETE, &format!("/api/admin/users/{user_id}"))
                .cookie(admin.clone()),
        )
        .await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.json()["message"], "用户已删除");
    let reset_deleted = env
        .send(
            TestRequest::new(
                Method::POST,
                &format!("/api/admin/users/{user_id}/reset-password"),
            )
            .cookie(admin.clone()),
        )
        .await;
    assert_eq!(reset_deleted.status, StatusCode::BAD_REQUEST);
    assert_eq!(reset_deleted.text(), "已删除用户不能重置密码");

    // 恢复；未删除不能恢复。
    let restored = env
        .send(
            TestRequest::new(Method::POST, &format!("/api/admin/users/{user_id}/restore"))
                .cookie(admin.clone()),
        )
        .await;
    assert_eq!(restored.status, StatusCode::OK);
    assert_eq!(restored.json()["message"], "用户已恢复");
    let restore_again = env
        .send(
            TestRequest::new(Method::POST, &format!("/api/admin/users/{user_id}/restore"))
                .cookie(admin.clone()),
        )
        .await;
    assert_eq!(restore_again.status, StatusCode::BAD_REQUEST);
    assert_eq!(restore_again.text(), "该用户未被删除");

    // 管理员不能删除自己；不存在的用户 404。
    let admin_id: i32 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let self_delete = env
        .send(
            TestRequest::new(Method::DELETE, &format!("/api/admin/users/{admin_id}"))
                .cookie(admin.clone()),
        )
        .await;
    assert_eq!(self_delete.status, StatusCode::BAD_REQUEST);
    assert_eq!(self_delete.text(), "不能删除当前登录管理员账号");
    let missing = env
        .send(TestRequest::new(Method::DELETE, "/api/admin/users/999999").cookie(admin))
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.text(), "用户不存在");
}

#[tokio::test]
async fn role_change_updates_admin_access_without_relogin() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    let admin_id = insert_user(&env.pool, "admin", "admin@example.com", &hash, "ADMIN").await;
    let token = login_admin(&env, "admin", "admin-pass").await;

    // 数据库角色降级：权限只看当前 DB，且清除二级验证状态。
    sqlx::query("UPDATE users SET role = 'USER' WHERE id = $1")
        .bind(admin_id)
        .execute(&env.pool)
        .await
        .unwrap();

    let forbidden = env
        .send(TestRequest::new(Method::GET, "/api/admin/users").cookie(token.clone()))
        .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN, "非管理员应为 403");
    assert_eq!(
        forbidden.json()["error"],
        "Forbidden",
        "非管理员 403 应为 Boot 默认 JSON"
    );

    // Re-promotion uses the current database role without secondary verification.
    sqlx::query("UPDATE users SET role = 'ADMIN' WHERE id = $1")
        .bind(admin_id)
        .execute(&env.pool)
        .await
        .unwrap();
    let promoted = env
        .send(TestRequest::new(Method::GET, "/api/admin/users").cookie(token))
        .await;
    assert_eq!(promoted.status, StatusCode::OK);
}

#[tokio::test]
async fn admin_access_needs_only_login_and_legacy_verify_ignores_passcodes() {
    let env = TestEnv::new().await;
    let hash = PasswordHasher::new(4, 1)
        .hash("admin-pass".into())
        .await
        .unwrap();
    insert_user(&env.pool, "admin", "admin@example.com", &hash, "ADMIN").await;
    let token = login(&env, "admin", "admin-pass")
        .await
        .session_cookie()
        .unwrap();
    // No verify call or secondAt field is necessary for either admin queue.
    for path in [
        "/api/admin/users",
        "/api/admin/markers/pending",
        "/api/admin/markers/pending-edits",
        "/api/admin/markers/pending-images",
    ] {
        assert_eq!(
            env.send(TestRequest::new(Method::GET, path).cookie(token.clone()))
                .await
                .status,
            StatusCode::OK,
            "{path}"
        );
    }
    for body in [
        serde_json::json!({}),
        serde_json::json!({"passcode": "wrong"}),
    ] {
        let response = env
            .send(TestRequest::json(Method::POST, "/api/admin/verify", body).cookie(token.clone()))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.body.is_empty());
    }
}

#[tokio::test]
async fn admin_users_paging_search_and_soft_delete_shape() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let admin_hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    insert_user(
        &env.pool,
        "admin",
        "admin@example.com",
        &admin_hash,
        "ADMIN",
    )
    .await;
    let user_hash = hasher.hash("user-pass".to_string()).await.unwrap();
    insert_user(
        &env.pool,
        "searchable-one",
        "one@example.com",
        &user_hash,
        "USER",
    )
    .await;
    let two = insert_user(
        &env.pool,
        "searchable-two",
        "two@example.com",
        &user_hash,
        "USER",
    )
    .await;
    insert_user(&env.pool, "other", "other@example.com", &user_hash, "USER").await;

    let admin = login_admin(&env, "admin", "admin-pass").await;
    let page = env
        .send(TestRequest::new(Method::GET, "/api/admin/users?page=0&size=2").cookie(admin.clone()))
        .await;
    assert_eq!(page.status, StatusCode::OK);
    let data = page.json();
    assert_eq!(data["page"], 0);
    assert_eq!(data["size"], 2);
    assert_eq!(data["totalElements"], 4);
    assert_eq!(data["totalPages"], 2);
    assert_eq!(data["items"].as_array().unwrap().len(), 2);
    let item = &data["items"][0];
    assert!(item.get("id").is_some());
    assert!(item.get("role").is_some());
    assert!(item.get("deleted").is_some());
    assert!(item.get("deletedAt").is_some() || item["deletedAt"].is_null());

    let search = env
        .send(TestRequest::new(Method::GET, "/api/admin/users?q=searchable").cookie(admin.clone()))
        .await;
    assert_eq!(search.status, StatusCode::OK);
    assert_eq!(search.json()["totalElements"], 2);

    // 软删除后列表可见 deleted=true 且 deletedAt 非空。
    let delete = env
        .send(
            TestRequest::new(Method::DELETE, &format!("/api/admin/users/{two}"))
                .cookie(admin.clone()),
        )
        .await;
    assert_eq!(delete.status, StatusCode::OK);
    let after = env
        .send(
            TestRequest::new(Method::GET, "/api/admin/users?q=searchable-two")
                .cookie(admin.clone()),
        )
        .await;
    let item = &after.json()["items"][0];
    assert_eq!(item["deleted"], true);
    assert!(!item["deletedAt"].is_null(), "软删除后 deletedAt 应非空");

    // size 夹取到上限 100，且负 page 归 0。
    let clamped = env
        .send(TestRequest::new(Method::GET, "/api/admin/users?size=1000&page=-3").cookie(admin))
        .await;
    assert_eq!(clamped.status, StatusCode::OK);
    assert_eq!(clamped.json()["page"], 0);
    assert_eq!(clamped.json()["size"], 100);
}

#[tokio::test]
async fn profile_patch_handles_null_and_blank_fields() {
    let env = TestEnv::new().await;
    let created = register(&env, "carol", "carol@example.com").await;
    let token = created.session_cookie().unwrap();

    // nickname 空白回退用户名；pronouns 空白置 null；signature 缺失保持原值。
    let signature = env
        .send(
            TestRequest::json(
                Method::PATCH,
                "/api/me",
                serde_json::json!({ "signature": "hello" }),
            )
            .cookie(token.clone()),
        )
        .await;
    assert_eq!(signature.status, StatusCode::OK);
    assert_eq!(signature.json()["data"]["signature"], "hello");

    let patched = env
        .send(
            TestRequest::json(
                Method::PATCH,
                "/api/me",
                serde_json::json!({ "nickname": "   ", "pronouns": "   " }),
            )
            .cookie(token.clone()),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK);
    let data = patched.json();
    assert_eq!(data["data"]["nickname"], "carol");
    assert!(data["data"]["pronouns"].is_null());
    assert_eq!(data["data"]["signature"], "hello");
}

#[tokio::test]
async fn security_entry_shapes_and_form_rejection() {
    let env = TestEnv::new().await;

    // 未认证受保护接口：固定 401 安全入口体。
    for uri in ["/api/me", "/api/admin/users"] {
        let response = env.send(TestRequest::new(Method::GET, uri)).await;
        assert_eq!(response.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(response.json()["message"], "Spring Security Error");
    }

    // 表单内容类型被拒绝（415）。
    let form = env
        .send(
            TestRequest::new(Method::POST, "/api/login")
                .content_type("application/x-www-form-urlencoded")
                .header("origin", ALLOWED_ORIGIN),
        )
        .await;
    assert_eq!(form.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // application/jsonp 不得被当作 JSON 接受。
    let jsonp = env
        .send(
            TestRequest::new(Method::POST, "/api/login")
                .content_type("application/jsonp")
                .header("origin", ALLOWED_ORIGIN),
        )
        .await;
    assert_eq!(jsonp.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // 超过 JSON 提取器上限的请求体返回 413（而不是笼统 400）。
    let huge = serde_json::json!({ "username": "x".repeat(70 * 1024), "password": "y" });
    let oversized = env
        .send(TestRequest::json(Method::POST, "/api/login", huge).header("origin", ALLOWED_ORIGIN))
        .await;
    assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);

    // 非管理员登录访问管理接口：Spring Boot 默认 403 JSON。
    register(&env, "plain", "plain@example.com").await;
    let token = login(&env, "plain", "test-password")
        .await
        .session_cookie()
        .unwrap();
    let forbidden = env
        .send(TestRequest::new(Method::GET, "/api/admin/users").cookie(token))
        .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.json()["error"], "Forbidden");
    assert_eq!(forbidden.json()["path"], "/api/admin/users");
}

#[tokio::test]
async fn write_origin_rules_and_native_app_compatibility() {
    let env = TestEnv::new().await;
    let body = serde_json::json!({ "username": "nobody", "password": "wrong" });

    // 携带非法 Origin 的写请求被拒绝。
    let cross = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("origin", "https://evil.example.com"),
        )
        .await;
    assert_eq!(cross.status, StatusCode::FORBIDDEN);

    // Origin 为 null 同样拒绝。
    let null_origin = env
        .send(TestRequest::json(Method::POST, "/api/login", body.clone()).header("origin", "null"))
        .await;
    assert_eq!(null_origin.status, StatusCode::FORBIDDEN);

    // 合法 Origin 放行到业务层（凭据失败 401 而非 403）。
    let allowed = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("origin", ALLOWED_ORIGIN),
        )
        .await;
    assert_eq!(allowed.status, StatusCode::UNAUTHORIZED);

    // 无 Origin 但明确跨站 Fetch Metadata 拒绝。
    let fetch_cross = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("sec-fetch-site", "cross-site"),
        )
        .await;
    assert_eq!(fetch_cross.status, StatusCode::FORBIDDEN);

    // same-site 声明的请求仍要校验 Referer：恶意 Referer 拒绝。
    let same_site_bad_referer = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("sec-fetch-site", "same-site")
                .header("referer", "https://evil.example.com/login"),
        )
        .await;
    assert_eq!(same_site_bad_referer.status, StatusCode::FORBIDDEN);

    // 非法 FetchMetadata 值不得当可信 same-origin。
    let invalid_fetch = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("sec-fetch-site", "banana"),
        )
        .await;
    assert_eq!(invalid_fetch.status, StatusCode::FORBIDDEN);

    // 无 Origin、有合法 Referer：通过。
    let referer_ok = env
        .send(
            TestRequest::json(Method::POST, "/api/login", body.clone())
                .header("referer", &format!("{ALLOWED_ORIGIN}/login")),
        )
        .await;
    assert_eq!(referer_ok.status, StatusCode::UNAUTHORIZED);

    // 完全无浏览器头的原生 App 请求：通过。
    let native = env
        .send(TestRequest::json(Method::POST, "/api/login", body))
        .await;
    assert_eq!(native.status, StatusCode::UNAUTHORIZED);

    // GET 不受来源校验影响。
    let get = env
        .send(TestRequest::new(Method::GET, "/api/me").header("origin", "https://evil.example.com"))
        .await;
    assert_eq!(get.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn register_rate_limit_and_redis_failure() {
    let env = TestEnv::custom(|config| {
        config.register_rate_limit_max = 2;
        config.register_rate_limit_window = Duration::from_secs(600);
        // 缩短坏依赖的超时，避免用例等待过久。
        config.redis_command_timeout = Duration::from_millis(200);
    })
    .await;

    assert_eq!(
        register(&env, "limited-a", "limited-a@example.com")
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        register(&env, "limited-b", "limited-b@example.com")
            .await
            .status,
        StatusCode::OK
    );
    let third = register(&env, "limited-c", "limited-c@example.com").await;
    assert_eq!(third.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(third.json()["code"], 429);

    // Redis 不可用时限流返回 503，绝不无上限放行。
    let broken = env.router_with_redis(unreachable_redis());
    let response = send_with(
        &broken,
        TestRequest::json(
            Method::POST,
            "/api/register",
            register_body(&env, "broken", "broken@example.com").await,
        ),
    )
    .await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn trusted_proxy_forwarded_ip_partitions_rate_limit() {
    let connect = IpAddr::from([10, 0, 0, 5]);
    let env = TestEnv::custom(|config| {
        config.register_rate_limit_max = 1;
        config.trusted_proxies = vec![connect];
    })
    .await;

    // 可信代理：不同 XFF 首段是不同限流身份。
    let first = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "xff-a", "xff-a@example.com").await,
            )
            .connect_ip(connect)
            .header("x-forwarded-for", "1.1.1.1, 10.0.0.5"),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "xff-b", "xff-b@example.com").await,
            )
            .connect_ip(connect)
            .header("x-forwarded-for", "2.2.2.2"),
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);
    let repeat = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "xff-c", "xff-c@example.com").await,
            )
            .connect_ip(connect)
            .header("x-forwarded-for", "1.1.1.1"),
        )
        .await;
    assert_eq!(repeat.status, StatusCode::TOO_MANY_REQUESTS);

    // 不可信连接伪造 XFF 无效，仍按连接 IP 计数。
    let env = TestEnv::custom(|config| {
        config.register_rate_limit_max = 1;
    })
    .await;
    let untrusted = IpAddr::from([203, 0, 113, 9]);
    let a = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "spoof-a", "spoof-a@example.com").await,
            )
            .connect_ip(untrusted)
            .header("x-forwarded-for", "1.1.1.1"),
        )
        .await;
    assert_eq!(a.status, StatusCode::OK);
    let b = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "spoof-b", "spoof-b@example.com").await,
            )
            .connect_ip(untrusted)
            .header("x-forwarded-for", "2.2.2.2"),
        )
        .await;
    assert_eq!(b.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn changepassword_missing_params_and_wrong_old() {
    let env = TestEnv::new().await;
    let created = register(&env, "dave", "dave@example.com").await;
    let token = created.session_cookie().unwrap();

    let missing = env
        .send(
            TestRequest::json(Method::POST, "/api/me/password", serde_json::json!({}))
                .cookie(token.clone()),
        )
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.json()["message"], "缺少参数");

    let wrong = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/me/password",
                serde_json::json!({ "oldPassword": "nope", "newPassword": "new-password" }),
            )
            .cookie(token),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert_eq!(wrong.json()["message"], "原密码错误或新密码不合法");
}

#[tokio::test]
async fn session_store_cas_invalidation_does_not_delete_advanced_version() {
    let redis = connect_redis().await;
    let namespace = format!("lycoris:test:{}:cas", Uuid::new_v4().simple());
    let store = SessionStore::new(
        redis,
        namespace,
        Duration::from_secs(300),
        Duration::from_secs(2),
    );

    let v0 = NewSession {
        user_id: 42,
        session_version: 0,
        role: "USER".to_string(),
    };
    let token = store.create(&v0, None).await.expect("创建会话失败");
    // B 完成改密并推进同一个 cookie 的会话版本到 1。
    let now = chrono::Utc::now().timestamp_millis();
    assert_eq!(
        store
            .begin_password_change(&token, &v0, "nonce-b", now + 60_000, now)
            .await
            .expect("begin 失败"),
        TransitionState::Marked
    );
    assert!(
        store
            .complete_password_change(&token, &v0, "nonce-b", 1)
            .await
            .expect("complete 失败")
    );
    // A 的旧快照（v0）CAS 失效必须失败，不能删掉 B 刚保留的当前登录。
    assert!(
        !store
            .invalidate_if_matches(&token, &v0, now)
            .await
            .expect("CAS 失效调用失败"),
        "旧快照不得删除已推进的新版本"
    );
    let record = store
        .read(&token)
        .await
        .expect("读取失败")
        .expect("会话应仍在");
    assert_eq!(record.session_version, 1);

    // 快照一致时才删除。
    let v1 = NewSession {
        user_id: 42,
        session_version: 1,
        role: "USER".to_string(),
    };
    assert!(
        store
            .invalidate_if_matches(&token, &v1, now)
            .await
            .expect("CAS 失效失败")
    );
    assert!(store.read(&token).await.expect("读取失败").is_none());
}

#[tokio::test]
async fn concurrent_me_requests_survive_password_change() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("old-pass".to_string()).await.unwrap();
    insert_user(
        &env.pool,
        "concurrent",
        "concurrent@example.com",
        &hash,
        "USER",
    )
    .await;

    let token = login(&env, "concurrent", "old-pass")
        .await
        .session_cookie()
        .unwrap();
    // 第二个会话（另一设备）应在改密后失效。
    let other = login(&env, "concurrent", "old-pass")
        .await
        .session_cookie()
        .unwrap();

    // 并发发起多个 /me，同时改密；当前会话不能被误判为 401。
    let mut handles = Vec::new();
    for _ in 0..6 {
        let router = env.router.clone();
        let token = token.clone();
        handles.push(tokio::spawn(async move {
            send_with(
                &router,
                TestRequest::new(Method::GET, "/api/me").cookie(token),
            )
            .await
        }));
    }
    let change = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/me/password",
                serde_json::json!({ "oldPassword": "old-pass", "newPassword": "brand-new-pass" }),
            )
            .cookie(token.clone()),
        )
        .await;
    assert_eq!(change.status, StatusCode::OK);
    for handle in handles {
        let response = handle.await.unwrap();
        // 转换窗口内可以是 200（已完成推进）或 503（pending 进行中），但绝不能是 401 或被删除。
        assert!(
            matches!(
                response.status,
                StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
            ),
            "改密期间的当前会话 /me 不应变为 401/被删除，实际 {}",
            response.status
        );
    }
    // 改密后当前会话仍有效，其它会话失效。
    assert_eq!(
        env.send(TestRequest::new(Method::GET, "/api/me").cookie(token))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        env.send(TestRequest::new(Method::GET, "/api/me").cookie(other))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn session_redis_failure_returns_503_not_timeout() {
    let env = TestEnv::custom(|config| {
        config.redis_command_timeout = Duration::from_millis(200);
    })
    .await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("user-pass".to_string()).await.unwrap();
    insert_user(
        &env.pool,
        "redisfail",
        "redisfail@example.com",
        &hash,
        "USER",
    )
    .await;

    let router = env.router_with_redis(unreachable_redis());
    // 带 Cookie 的受保护请求：会话读取失败 -> 503，而不是等到全局 408。
    let me = send_with(
        &router,
        TestRequest::new(Method::GET, "/api/me").cookie("deadbeef".repeat(8)),
    )
    .await;
    assert_eq!(me.status, StatusCode::SERVICE_UNAVAILABLE);

    // 登录：PG 校验通过但会话创建失败 -> 503。
    let login = send_with(
        &router,
        TestRequest::json(
            Method::POST,
            "/api/login",
            serde_json::json!({ "username": "redisfail", "password": "user-pass" }),
        ),
    )
    .await;
    assert_eq!(login.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn register_rotates_existing_session() {
    let env = TestEnv::new().await;
    let first = register(&env, "original", "original@example.com").await;
    let old_token = first.session_cookie().unwrap();

    // 已登录用户再注册另一个账号：必须原子替换旧会话。
    let second = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/register",
                register_body(&env, "second-account", "second@example.com").await,
            )
            .cookie(old_token.clone()),
        )
        .await;
    assert_eq!(second.status, StatusCode::OK);
    let new_token = second.session_cookie().expect("注册应轮换会话");
    assert_ne!(old_token, new_token, "注册应产生新的会话标识");

    assert_eq!(
        env.send(TestRequest::new(Method::GET, "/api/me").cookie(old_token))
            .await
            .status,
        StatusCode::UNAUTHORIZED,
        "注册前的旧会话应失效"
    );
    let me = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(new_token))
        .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["data"]["username"], "second-account");
}

#[tokio::test]
async fn logout_delete_failure_returns_503_and_keeps_session() {
    let env = TestEnv::custom(|config| {
        config.redis_command_timeout = Duration::from_millis(500);
    })
    .await;
    let admin_redis = connect_redis().await;
    let acl_name = format!("lycoris_test_acl_{}", Uuid::new_v4().simple());
    let acl_password = Uuid::new_v4().simple().to_string();
    acl_setuser_deny_del(&admin_redis, &acl_name, &acl_password).await;

    let acl_url = format!("redis://{acl_name}:{acl_password}@127.0.0.1:56379");
    let acl_client = connect_redis_url(&acl_url)
        .await
        .expect("无法以受限 ACL 用户连接测试 Redis");
    let router = env.router_with_redis(acl_client);

    // 正常客户端完成注册/验证码消费，再通过受限客户端验证退出故障。
    let created = send_with(
        &env.router,
        TestRequest::json(
            Method::POST,
            "/api/register",
            register_body(&env, "logout-acl", "logout-acl@example.com").await,
        ),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK);
    let token = created.session_cookie().unwrap();

    // logout 的 DEL 被 ACL 拒绝 -> 503，且不得下发“清除成功”的 Cookie。
    let logout = send_with(
        &router,
        TestRequest::new(Method::POST, "/api/logout").cookie(token.clone()),
    )
    .await;
    assert_eq!(
        logout.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "DEL 失败必须返回 503，不能声称退出成功"
    );
    let cleared = logout
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("Max-Age=0"));
    assert!(!cleared, "删除失败时不得声称已清除会话");

    // 旧 token 仍未被撤销：管理端 Redis 仍能看到会话记录。
    let store = SessionStore::new(
        admin_redis.clone(),
        env.config.session_namespace.clone(),
        env.config.session_ttl,
        Duration::from_secs(2),
    );
    let exists: i64 = admin_redis
        .exists(store.key_for(&token))
        .await
        .expect("查询会话 key 失败");
    assert_eq!(exists, 1, "删除失败后旧 token 仍应存在");

    acl_deluser(&admin_redis, &acl_name).await;
}

/// 直接读取测试账号的 id/version/role，供测试用公开存储方法调度改密窗口。
async fn user_snapshot(env: &TestEnv, username: &str) -> NewSession {
    let (user_id, session_version, role): (i32, i64, String) =
        sqlx::query_as("SELECT id, session_version, role FROM users WHERE username = $1")
            .bind(username)
            .fetch_one(&env.pool)
            .await
            .expect("读取用户快照失败");
    NewSession {
        user_id,
        session_version,
        role,
    }
}

#[tokio::test]
async fn password_change_pending_window_is_503_not_401() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("old-pass".to_string()).await.unwrap();
    insert_user(&env.pool, "pending", "pending@example.com", &hash, "USER").await;

    let token_current = login(&env, "pending", "old-pass")
        .await
        .session_cookie()
        .unwrap();
    // 另一台设备的会话，保持旧版本。
    let token_old = login(&env, "pending", "old-pass")
        .await
        .session_cookie()
        .unwrap();
    let snapshot = user_snapshot(&env, "pending").await;

    // begin 后模拟“改密已提交 PG、Redis 尚未推进”的窗口。
    let now = chrono::Utc::now().timestamp_millis();
    assert_eq!(
        env.state
            .session
            .begin_password_change(&token_current, &snapshot, "nonce-w", now + 60_000, now)
            .await
            .unwrap(),
        TransitionState::Marked
    );
    sqlx::query("UPDATE users SET session_version = session_version + 1 WHERE id = $1")
        .bind(snapshot.user_id)
        .execute(&env.pool)
        .await
        .unwrap();

    // 超过旧的 18ms 有界重读窗口：当前会话应 503（转换进行中），既不 401 也不被删除。
    tokio::time::sleep(Duration::from_millis(150)).await;
    let during = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token_current.clone()))
        .await;
    assert_eq!(
        during.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "pending 窗口内当前会话应为 503"
    );

    // 完成推进后同一 Cookie 恢复 200。
    assert!(
        env.state
            .session
            .complete_password_change(
                &token_current,
                &snapshot,
                "nonce-w",
                snapshot.session_version + 1
            )
            .await
            .unwrap()
    );
    let after = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token_current))
        .await;
    assert_eq!(after.status, StatusCode::OK);

    // 旧设备没有 pending：版本失配后最终 401，不会被复活。
    let old = env
        .send(TestRequest::new(Method::GET, "/api/me").cookie(token_old))
        .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn password_change_transition_states() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("old-pass".to_string()).await.unwrap();
    insert_user(&env.pool, "states", "states@example.com", &hash, "USER").await;
    let token = login(&env, "states", "old-pass")
        .await
        .session_cookie()
        .unwrap();
    let snapshot = user_snapshot(&env, "states").await;
    let now = chrono::Utc::now().timestamp_millis();

    assert_eq!(
        env.state
            .session
            .begin_password_change(&token, &snapshot, "n1", now + 60_000, now)
            .await
            .unwrap(),
        TransitionState::Marked
    );
    // 已有进行中的转换：再次 begin 返回 AlreadyPending，HTTP 改密返回 409。
    assert_eq!(
        env.state
            .session
            .begin_password_change(&token, &snapshot, "n2", now + 60_000, now)
            .await
            .unwrap(),
        TransitionState::AlreadyPending
    );
    let busy = env
        .send(
            TestRequest::json(
                Method::POST,
                "/api/me/password",
                serde_json::json!({ "oldPassword": "old-pass", "newPassword": "new-pass-1" }),
            )
            .cookie(token.clone()),
        )
        .await;
    assert_eq!(busy.status, StatusCode::CONFLICT);

    // nonce 不匹配的 complete 不生效。
    assert!(
        !env.state
            .session
            .complete_password_change(&token, &snapshot, "wrong-nonce", 1)
            .await
            .unwrap()
    );
    // 按 nonce 取消后 pending 清除。
    assert!(
        env.state
            .session
            .cancel_password_change(&token, &snapshot, "n1")
            .await
            .unwrap()
    );
    let record = env.state.session.read(&token).await.unwrap().unwrap();
    assert!(record.pending_until.is_none());
    assert_eq!(record.session_version, snapshot.session_version);

    // begin 后退出，再 complete 不能复活会话。
    assert_eq!(
        env.state
            .session
            .begin_password_change(&token, &snapshot, "n3", now + 60_000, now)
            .await
            .unwrap(),
        TransitionState::Marked
    );
    let logout = env
        .send(TestRequest::new(Method::POST, "/api/logout").cookie(token.clone()))
        .await;
    assert_eq!(logout.status, StatusCode::OK);
    assert!(
        !env.state
            .session
            .complete_password_change(&token, &snapshot, "n3", 1)
            .await
            .unwrap(),
        "退出后 complete 不得复活会话"
    );
    assert!(env.state.session.read(&token).await.unwrap().is_none());
}

#[tokio::test]
async fn expired_pending_does_not_block_invalidation() {
    let redis = connect_redis().await;
    let namespace = format!("lycoris:test:{}:expire", Uuid::new_v4().simple());
    let store = SessionStore::new(
        redis,
        namespace,
        Duration::from_secs(300),
        Duration::from_secs(2),
    );
    let base = NewSession {
        user_id: 9,
        session_version: 0,
        role: "USER".to_string(),
    };
    let token = store.create(&base, None).await.unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    // 已过期的 pending 允许被覆盖；随后失效删除应成功。
    assert_eq!(
        store
            .begin_password_change(&token, &base, "old", now - 1_000, now)
            .await
            .unwrap(),
        TransitionState::Marked
    );
    assert!(
        store
            .invalidate_if_matches(&token, &base, now)
            .await
            .unwrap(),
        "过期 pending 不应阻止失效删除"
    );
}

#[tokio::test]
async fn role_missing_admin_403_is_boot_json() {
    let env = TestEnv::new().await;
    let created = register(&env, "plain403", "plain403@example.com").await;
    let token = created.session_cookie().unwrap();

    let checks = [
        (
            TestRequest::new(Method::GET, "/api/admin/users"),
            "/api/admin/users",
        ),
        (
            TestRequest::json(
                Method::POST,
                "/api/admin/verify",
                serde_json::json!({ "passcode": "x" }),
            ),
            "/api/admin/verify",
        ),
    ];
    for (request, uri) in checks {
        let response = env.send(request.cookie(token.clone())).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{uri}");
        let body = response.json();
        assert_eq!(body["status"], 403, "{uri}");
        assert_eq!(body["error"], "Forbidden", "{uri}");
        assert_eq!(body["path"], uri, "{uri}");
        let timestamp = body["timestamp"].as_str().expect("timestamp 应为字符串");
        chrono::DateTime::parse_from_rfc3339(timestamp).expect("timestamp 应可解析");
    }
}

#[tokio::test]
async fn legacy_second_factor_timestamp_does_not_gate_admin_access() {
    let env = TestEnv::new().await;
    let hasher = PasswordHasher::new(4, 1);
    let hash = hasher.hash("admin-pass".to_string()).await.unwrap();
    insert_user(&env.pool, "admin", "admin@example.com", &hash, "ADMIN").await;
    let token = login_admin(&env, "admin", "admin-pass").await;

    // A historical session timestamp no longer grants or blocks admin access.
    let admin_redis = connect_redis().await;
    let store = SessionStore::new(
        admin_redis.clone(),
        env.config.session_namespace.clone(),
        env.config.session_ttl,
        Duration::from_secs(2),
    );
    let future = chrono::Utc::now().timestamp_millis() + 3_600_000;
    admin_redis
        .hset::<i64, _, _>(store.key_for(&token), ("secondAt", future.to_string()))
        .await
        .expect("写入未来 secondAt 失败");

    let response = env
        .send(TestRequest::new(Method::GET, "/api/admin/users").cookie(token))
        .await;
    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn role_sync_cas_loses_to_concurrent_advance() {
    let redis = connect_redis().await;
    let namespace = format!("lycoris:test:{}:role", Uuid::new_v4().simple());
    let store = SessionStore::new(
        redis,
        namespace,
        Duration::from_secs(300),
        Duration::from_secs(2),
    );
    let base = NewSession {
        user_id: 7,
        session_version: 0,
        role: "USER".to_string(),
    };
    let token = store.create(&base, None).await.unwrap();

    // 另一个请求先把版本推进到 1；旧快照的 role 同步 CAS 必须失败且不改动记录。
    let now = chrono::Utc::now().timestamp_millis();
    assert_eq!(
        store
            .begin_password_change(&token, &base, "nonce-r", now + 60_000, now)
            .await
            .unwrap(),
        TransitionState::Marked
    );
    assert!(
        store
            .complete_password_change(&token, &base, "nonce-r", 1)
            .await
            .unwrap()
    );
    assert!(
        !store
            .sync_role(&token, &base, "ADMIN")
            .await
            .expect("role 同步调用失败"),
        "旧快照不应同步成功"
    );
    let record = store.read(&token).await.unwrap().unwrap();
    assert_eq!(record.role, "USER");
    assert_eq!(record.session_version, 1);
}
