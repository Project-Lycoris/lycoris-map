//! 阶段 4 发布运行参数补齐的真实 PG / Redis 集成测试。
//!
//! 验证数据库语句/锁超时真正接到服务连接、维护连接不继承服务短超时，以及 SQLSTATE
//! 57014/55P03 在真实 Router（`tower::ServiceExt::oneshot`，**不是**真实 TCP 服务）上受控
//! 映射 503（保留 CORS 与请求 ID），解锁后正常成功。每个用例使用 UUID 临时库与独立 Redis
//! 命名空间，只连接回环合成服务。

mod common;

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use common::{TempDatabase, connect_redis, test_redis_url, unique_cache_namespace};
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::db;
use lycoris_backend::modules::markers::cache::MarkerCache;
use lycoris_backend::modules::markers::write::MarkerWriteService;
use lycoris_backend::modules::markers::write_model::{Actor, WriteError};
use lycoris_backend::password::PasswordHasher;
use serde_json::{Value, json};
use sqlx::{Acquire as _, PgPool};
use tower::ServiceExt;
use uuid::Uuid;

const ALLOWED_ORIGIN: &str = "https://app.example.com";

fn serve_config(temp: &TempDatabase, statement_ms: u64, lock_ms: u64) -> Config {
    let mut config = Config::new(temp.url(), test_redis_url());
    config.db_statement_timeout = Duration::from_millis(statement_ms);
    config.db_lock_timeout = Duration::from_millis(lock_ms);
    config
}

#[tokio::test]
async fn serve_pool_applies_statement_and_lock_timeouts_per_connection() {
    let temp = TempDatabase::create().await;
    let config = serve_config(&temp, 250, 125);
    let pool = db::connect_serve_pool(&config)
        .await
        .expect("构建服务池失败");

    // 必须**同时**持有两条物理连接再断言：顺序 acquire/drop 可能复用同一条连接，
    // 无法证明 after_connect 对每条新连接都生效。
    let mut first = pool.acquire().await.expect("取第一条连接失败");
    let mut second = pool.acquire().await.expect("取第二条连接失败");
    let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *first)
        .await
        .expect("查询第一条 backend pid 失败");
    let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *second)
        .await
        .expect("查询第二条 backend pid 失败");
    assert_ne!(first_pid, second_pid, "两条连接必须是不同物理 backend");

    for (name, connection) in [("第一条", &mut *first), ("第二条", &mut *second)] {
        let (statement, unit): (String, String) = sqlx::query_as(
            "SELECT setting, unit FROM pg_settings WHERE name = 'statement_timeout'",
        )
        .fetch_one(&mut *connection)
        .await
        .expect("查询 statement_timeout 失败");
        assert_eq!(statement, "250", "{name}连接语句超时未生效");
        assert_eq!(unit, "ms");
        let (lock, unit): (String, String) =
            sqlx::query_as("SELECT setting, unit FROM pg_settings WHERE name = 'lock_timeout'")
                .fetch_one(&mut *connection)
                .await
                .expect("查询 lock_timeout 失败");
        assert_eq!(lock, "125", "{name}连接锁超时未生效");
        assert_eq!(unit, "ms");
    }
    drop(first);
    drop(second);
    pool.close().await;
}

#[tokio::test]
async fn serve_pool_cancels_overlong_statement_and_connection_recovers() {
    let temp = TempDatabase::create().await;
    let config = serve_config(&temp, 200, 100);
    let pool = db::connect_serve_pool(&config)
        .await
        .expect("构建服务池失败");

    let mut connection = pool.acquire().await.expect("取连接失败");
    let started = Instant::now();
    let error = sqlx::query("SELECT pg_sleep(5)")
        .execute(&mut *connection)
        .await
        .expect_err("超过配置的语句应被 PG 取消");
    let elapsed = started.elapsed();
    assert!(
        db::is_timeout_sqlstate(&error),
        "应为 57014（query_canceled），实际 {error:?}"
    );
    // 时间上界宽容：只需明显早于 5 秒休眠完成，且不是瞬时误判。
    assert!(
        elapsed >= Duration::from_millis(50),
        "取消早于配置太多，可能误判: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "取消应在上界内完成，实际 {elapsed:?}"
    );

    // 同一连接在语句被取消后仍可复用。
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut *connection)
        .await
        .expect("取消后同连接 SELECT 1 应可用");
    assert_eq!(one, 1);
    drop(connection);
    pool.close().await;
}

#[tokio::test]
async fn lock_wait_timeout_reports_unavailable_without_partial_write() {
    let temp = TempDatabase::create().await;
    let config = serve_config(&temp, 2000, 150);
    // 夹具 schema 用维护池建立，避免用被测短超时池跑 PostGIS 基线建表，把夹具耗时误判为产品超时。
    let maintenance = db::connect_maintenance_pool(&config)
        .await
        .expect("构建维护池失败");
    lycoris_backend::migrate::run(&maintenance)
        .await
        .expect("迁移临时库失败");
    maintenance.close().await;
    let pool = db::connect_serve_pool(&config)
        .await
        .expect("构建服务池失败");
    let redis = connect_redis().await;
    let write = MarkerWriteService::new(
        pool.clone(),
        MarkerCache::new(redis, true, unique_cache_namespace()),
    );
    let owner = Actor::new("owner-public-1", "owner", false);
    let marker = seed_marker(&pool, "owner-public-1").await;

    // 另一连接持有行锁，业务事务在 lock_timeout 后以 55P03 失败。
    let mut blocker = pool.acquire().await.expect("取阻塞连接失败");
    let mut blocker_tx = blocker.begin().await.expect("开启阻塞事务失败");
    sqlx::query("SELECT id FROM map_markers WHERE id = $1 FOR UPDATE")
        .bind(marker)
        .fetch_one(&mut *blocker_tx)
        .await
        .expect("阻塞事务加锁失败");

    let started = Instant::now();
    let error = write
        .add_favorite(&owner, marker)
        .await
        .expect_err("行锁竞争应按锁超时失败");
    assert!(
        matches!(error, WriteError::Unavailable),
        "锁等待超时应映射 503，实际 {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "锁超时应及时返回，实际 {:?}",
        started.elapsed()
    );

    // 事务整体回滚：不得留下任何部分写入。
    let favorites: i64 =
        sqlx::query_scalar("SELECT count(*) FROM marker_favorites WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .expect("查询收藏失败");
    assert_eq!(favorites, 0, "失败事务不得留下部分写入");
    let version: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id = $1")
        .bind(marker)
        .fetch_one(&pool)
        .await
        .expect("查询点位版本失败");
    assert_eq!(version, 0, "点位不得被失败事务改动");

    // 释放锁后同一业务写入正常成功。
    blocker_tx.rollback().await.expect("释放阻塞事务失败");
    drop(blocker);
    write
        .add_favorite(&owner, marker)
        .await
        .expect("解锁后收藏应成功");
    let favorites: i64 =
        sqlx::query_scalar("SELECT count(*) FROM marker_favorites WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .expect("复查收藏失败");
    assert_eq!(favorites, 1);
    pool.close().await;
}

#[tokio::test]
async fn maintenance_pool_keeps_no_service_statement_timeout() {
    let temp = TempDatabase::create().await;
    let config = serve_config(&temp, 150, 100);
    let maintenance = db::connect_maintenance_pool(&config)
        .await
        .expect("构建维护池失败");

    let mut connection = maintenance.acquire().await.expect("取维护连接失败");
    let statement: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *connection)
        .await
        .expect("查询维护连接 statement_timeout 失败");
    assert_eq!(statement, "0", "维护连接不得继承服务语句超时");
    // 维护连接仍有独立、明确的迁移/接管锁等待上限（5s），不继承服务锁超时。
    let (lock, unit): (String, String) =
        sqlx::query_as("SELECT setting, unit FROM pg_settings WHERE name = 'lock_timeout'")
            .fetch_one(&mut *connection)
            .await
            .expect("查询维护连接 lock_timeout 失败");
    assert_eq!(lock, "5000", "维护连接应有独立 5s 锁等待上限");
    assert_eq!(unit, "ms");

    // 比服务超时更长的一条语句在维护连接上仍能完成。
    let started = Instant::now();
    sqlx::query("SELECT pg_sleep(0.4)")
        .execute(&mut *connection)
        .await
        .expect("维护连接不应被服务短超时取消");
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "维护语句应实际执行完，实际 {:?}",
        started.elapsed()
    );
    drop(connection);

    // 对照：同一配置的服务池会取消同样的语句。
    let serve = db::connect_serve_pool(&config)
        .await
        .expect("构建服务池失败");
    let error = sqlx::query("SELECT pg_sleep(0.4)")
        .execute(&serve)
        .await
        .expect_err("服务池应取消超过配置的语句");
    assert!(db::is_timeout_sqlstate(&error));

    serve.close().await;
    maintenance.close().await;
}

struct HttpEnv {
    _temp: TempDatabase,
    _upload: tempfile::TempDir,
    pool: PgPool,
    router: Router,
}

async fn http_env() -> HttpEnv {
    let temp = TempDatabase::create().await;
    let mut config = Config::new(temp.url(), test_redis_url());
    let unique = Uuid::new_v4().simple().to_string();
    config.session_namespace = format!("lycoris:test:{unique}:session");
    config.rate_limit_namespace = format!("lycoris:test:{unique}:ratelimit");
    config.marker_cache_namespace = format!("lycoris:test:{unique}:marker");
    config.bcrypt_cost = 4;
    config.write_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
    config.cors_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
    config.db_statement_timeout = Duration::from_millis(2000);
    config.db_lock_timeout = Duration::from_millis(150);
    let upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
    config.upload_dir = upload.path().to_path_buf();

    // 先用维护池建立 schema（含 PostGIS 基线），再建立被测的短超时服务池。
    let maintenance = db::connect_maintenance_pool(&config)
        .await
        .expect("构建维护池失败");
    lycoris_backend::migrate::run(&maintenance)
        .await
        .expect("迁移临时库失败");
    maintenance.close().await;
    let pool = db::connect_serve_pool(&config)
        .await
        .expect("构建服务池失败");
    let redis = connect_redis().await;
    let state = AppState::new(pool.clone(), redis, config).expect("构造 AppState 失败");
    let router = build_router(state);
    HttpEnv {
        _temp: temp,
        _upload: upload,
        pool,
        router,
    }
}

async fn insert_admin(pool: &PgPool, username: &str) -> Uuid {
    let password = PasswordHasher::new(4, 1)
        .hash(format!("{username}-pass"))
        .await
        .expect("生成密码哈希失败");
    let public_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (public_id, username, nickname, email, password, role, deleted, \
         session_version, row_version) VALUES ($1, $2, $2, $3, $4, 'ADMIN', false, 0, 0)",
    )
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.com"))
    .bind(password)
    .execute(pool)
    .await
    .expect("插入管理员失败");
    public_id
}

async fn seed_marker(pool: &PgPool, owner_public_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            open_time_start, open_time_end, review_status, username, user_public_id, version,
            last_edited_by, last_edited_by_public_id, last_edited_by_owner, created_at, updated_at)
         VALUES (1,2,'accessible_toilet','seed','desc','zh',true,true,NULL,NULL,'APPROVED',
                 'admin',$1,0,'admin',$1,true,now(),now())
         RETURNING id",
    )
    .bind(owner_public_id)
    .fetch_one(pool)
    .await
    .expect("插入点位失败")
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

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn cookie(&self) -> String {
        for value in self.headers.get_all(header::SET_COOKIE) {
            let raw = value.to_str().unwrap_or_default();
            if let Some(rest) = raw
                .split(';')
                .next()
                .and_then(|first| first.strip_prefix("LYCORIS_SESSION="))
            {
                return rest.to_string();
            }
        }
        panic!("登录应设置会话 Cookie");
    }
}

async fn send(
    env: &HttpEnv,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
) -> Resp {
    let mut builder = Request::builder().method(method).uri(uri);
    let payload = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(payload).unwrap();
    if let Some(token) = cookie {
        request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("LYCORIS_SESSION={token}")).unwrap(),
        );
    }
    for (name, value) in headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let response = env
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("调用路由失败");
    Resp::from(response).await
}

async fn login(env: &HttpEnv, username: &str) -> String {
    let response = send(
        env,
        Method::POST,
        "/api/login",
        Some(json!({ "username": username, "password": format!("{username}-pass") })),
        None,
        &[],
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.cookie()
}

async fn login_admin(env: &HttpEnv, username: &str) -> String {
    login(env, username).await
}

#[tokio::test]
async fn held_lock_write_returns_503_with_cors_and_request_id_then_succeeds() {
    let env = http_env().await;
    let public_id = insert_admin(&env.pool, "admin").await;
    let marker = seed_marker(&env.pool, &public_id.to_string()).await;
    let token = login_admin(&env, "admin").await;

    // 另一连接持有该点位行锁。
    let mut blocker = env.pool.acquire().await.expect("取阻塞连接失败");
    let mut blocker_tx = blocker.begin().await.expect("开启阻塞事务失败");
    sqlx::query("SELECT id FROM map_markers WHERE id = $1 FOR UPDATE")
        .bind(marker)
        .fetch_one(&mut *blocker_tx)
        .await
        .expect("阻塞事务加锁失败");

    let response = send(
        &env,
        Method::PATCH,
        &format!("/api/admin/markers/{marker}"),
        Some(json!({ "title": "被锁阻塞", "language": "zh" })),
        Some(&token),
        &[("origin", ALLOWED_ORIGIN)],
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "持锁写请求应 503，实际 {} {}",
        response.status,
        response.text()
    );
    assert_eq!(response.text(), "服务暂时不可用");
    assert_eq!(
        response
            .headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some(ALLOWED_ORIGIN),
        "503 仍应带 CORS 允许来源头"
    );
    let request_id = response
        .headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("503 响应应带 X-Request-ID");
    assert!(
        Uuid::parse_str(request_id).is_ok(),
        "请求 ID 应为服务端 UUID: {request_id}"
    );

    // 释放锁后同一写请求正常成功并真正写入。
    blocker_tx.rollback().await.expect("释放阻塞事务失败");
    let success = send(
        &env,
        Method::PATCH,
        &format!("/api/admin/markers/{marker}"),
        Some(json!({ "title": "解锁成功", "language": "zh" })),
        Some(&token),
        &[("origin", ALLOWED_ORIGIN)],
    )
    .await;
    assert_eq!(success.status, StatusCode::OK, "{}", success.text());
    let value: Value = serde_json::from_slice(&success.body).expect("成功响应应为 JSON");
    assert_eq!(value["title"], "解锁成功");
    assert_eq!(value["version"], json!(1));
    let title: String = sqlx::query_scalar("SELECT title FROM map_markers WHERE id = $1")
        .bind(marker)
        .fetch_one(&env.pool)
        .await
        .expect("复查点位标题失败");
    assert_eq!(title, "解锁成功");
}
