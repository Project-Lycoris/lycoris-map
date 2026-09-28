//! 阶段 3 点位写入/收藏/审核 HTTP 接入的真实 PG / Redis 集成测试。
//!
//! 通过真实 Router + 实际 `/api/login` 会话 Cookie 验证 18 条非图片路由：
//! 用户写与本人读取、`/api/markers/all`（管理员不二次）、`/api/admin/markers/**`
//! （管理员 + 二次验证）的匿名/普通/管理员矩阵，完整创建→审核→收藏→提案→审核→本地化
//! 生命周期，两条同基准提案的 HTTP 并发审核竞争，以及新建点位 `markImage` 收紧。
//! 每个用例使用 UUID 临时库与独立 Redis 命名空间，只清理自己的库。

mod common;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use common::{TempDatabase, connect_redis, test_redis_url};
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::{Config, REQUEST_BODY_LIMIT_BYTES};
use lycoris_backend::modules::markers::write_model::{
    Actor, MSG_MARK_IMAGE_UPLOAD_ONLY, MSG_STALE_VERSION, MarkerCreateRequest, WriteError,
};
use lycoris_backend::password::PasswordHasher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const ALLOWED_ORIGIN: &str = "https://app.example.com";

/// 兼容原有点位字段，并增加独立的软删除标志 `deactivated`。
const MARKER_KEYS: [&str; 26] = [
    "id",
    "version",
    "lat",
    "lng",
    "category",
    "title",
    "description",
    "sourceLanguage",
    "contentLanguage",
    "isPublic",
    "username",
    "userPublicId",
    "clientRequestId",
    "isActive",
    "deactivated",
    "openTimeStart",
    "openTimeEnd",
    "venueType",
    "hoursTimezone",
    "reviewStatus",
    "lastEditedBy",
    "lastEditedByPublicId",
    "lastEditedByOwner",
    "markImage",
    "createdAt",
    "updatedAt",
];

/// `pending-edits` 的 19 个字段。
const EDIT_PROPOSAL_KEYS: [&str; 19] = [
    "id",
    "markerId",
    "markerTitle",
    "lat",
    "lng",
    "category",
    "title",
    "description",
    "language",
    "isPublic",
    "isActive",
    "openTimeStart",
    "openTimeEnd",
    "venueType",
    "proposerUsername",
    "proposerPublicId",
    "proposerIsOwner",
    "status",
    "createdAt",
];

struct Env {
    _temp: TempDatabase,
    /// 每个用例独立的上传根目录；必须存活到整个 `Env` 结束，不能让 `TempDir` 提前 drop，
    /// 也不得使用默认真实 `uploads/`。
    _upload: tempfile::TempDir,
    pool: PgPool,
    state: AppState,
    router: Router,
}

impl Env {
    async fn new() -> Self {
        let (temp, pool) = TempDatabase::create_migrated().await;
        let redis = connect_redis().await;
        let upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
        let unique = Uuid::new_v4().simple().to_string();
        let mut config = Config::new(temp.url(), test_redis_url());
        config.session_namespace = format!("lycoris:test:{unique}:session");
        config.rate_limit_namespace = format!("lycoris:test:{unique}:ratelimit");
        config.marker_cache_namespace = format!("lycoris:test:{unique}:marker");
        config.bcrypt_cost = 4;
        config.write_allowed_origins = vec![HeaderValue::from_static(ALLOWED_ORIGIN)];
        config.upload_dir = upload.path().to_path_buf();
        let state = AppState::new(pool.clone(), redis, config).expect("构造 AppState 失败");
        let router = build_router(state.clone());
        Self {
            _temp: temp,
            _upload: upload,
            pool,
            state,
            router,
        }
    }
}

/// 直接插入带已知密码的账号，口令约定为 `{username}-pass`；返回 `public_id`。
async fn insert_user(env: &Env, username: &str, role: &str) -> Uuid {
    let password = PasswordHasher::new(4, 1)
        .hash(format!("{username}-pass"))
        .await
        .expect("生成密码哈希失败");
    let public_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (public_id, username, nickname, email, password, role, deleted, \
         session_version, row_version) VALUES ($1, $2, $2, $3, $4, $5, false, 0, 0)",
    )
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.com"))
    .bind(password)
    .bind(role)
    .execute(&env.pool)
    .await
    .expect("插入测试用户失败");
    public_id
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

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    fn cookie(&self) -> Option<String> {
        for value in self.headers.get_all(header::SET_COOKIE) {
            let raw = value.to_str().ok()?;
            let first = raw.split(';').next().unwrap_or_default();
            if let Some(rest) = first.strip_prefix("LYCORIS_SESSION=") {
                return Some(rest.to_string());
            }
        }
        None
    }
}

async fn send(
    env: &Env,
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

async fn get(env: &Env, uri: &str, cookie: Option<&str>) -> Resp {
    send(env, Method::GET, uri, None, cookie, &[]).await
}

async fn post(env: &Env, uri: &str, body: Value, cookie: Option<&str>) -> Resp {
    send(env, Method::POST, uri, Some(body), cookie, &[]).await
}

async fn post_empty(env: &Env, uri: &str, cookie: Option<&str>) -> Resp {
    send(env, Method::POST, uri, None, cookie, &[]).await
}

async fn patch(env: &Env, uri: &str, body: Value, cookie: Option<&str>) -> Resp {
    send(env, Method::PATCH, uri, Some(body), cookie, &[]).await
}

async fn delete(env: &Env, uri: &str, cookie: Option<&str>) -> Resp {
    send(env, Method::DELETE, uri, None, cookie, &[]).await
}

/// 发送原始请求体（用于非法 JSON 与自定义媒体类型的边界断言）。
async fn send_raw(
    env: &Env,
    method: Method,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    cookie: Option<&str>,
) -> Resp {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    if let Some(token) = cookie {
        request.headers_mut().insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("LYCORIS_SESSION={token}")).unwrap(),
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

/// 用真实 `/api/login` 会话取得 Cookie。
async fn login(env: &Env, username: &str) -> String {
    let response = post(
        env,
        "/api/login",
        json!({ "username": username, "password": format!("{username}-pass") }),
        None,
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "登录 {username} 失败: {}",
        response.text()
    );
    response.cookie().expect("登录应设置会话 Cookie")
}

/// Log in with an ordinary administrator account.
async fn login_admin(env: &Env, username: &str) -> String {
    login(env, username).await
}

fn create_body(title: &str, language: &str) -> Value {
    json!({
        "lat": 31.2304,
        "lng": 121.4737,
        "category": "accessible_toilet",
        "title": title,
        "description": "描述",
        "language": language,
    })
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("响应应为 JSON 对象")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

fn assert_marker_shape(value: &Value) {
    let mut expected: Vec<String> = MARKER_KEYS.iter().map(|key| key.to_string()).collect();
    expected.sort();
    assert_eq!(
        sorted_keys(value),
        expected,
        "Marker 字段与契约不一致: {value}"
    );
}

fn assert_edit_proposal_shape(value: &Value) {
    let mut expected: Vec<String> = EDIT_PROPOSAL_KEYS
        .iter()
        .map(|key| key.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        sorted_keys(value),
        expected,
        "编辑提案字段与契约不一致: {value}"
    );
}

fn assert_vary(headers: &HeaderMap) {
    let vary = headers
        .get(header::VARY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert_eq!(vary, "Accept-Language, X-App-Language", "缺少语言 Vary 头");
}

fn assert_security_entry(response: &Resp, uri: &str) {
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "匿名 {uri} 应 401"
    );
    assert_eq!(
        response.json(),
        json!({ "message": "Spring Security Error" }),
        "匿名 {uri} 形状错误"
    );
}

fn assert_boot_forbidden(response: &Resp, uri: &str) {
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "缺角色 {uri} 应 403"
    );
    let body = response.json();
    assert_eq!(body["status"], 403, "403 形状 status: {body}");
    assert_eq!(body["error"], "Forbidden", "403 形状 error: {body}");
    assert_eq!(body["path"], uri, "403 形状 path: {body}");
    assert!(
        body["timestamp"].as_str().is_some(),
        "403 应有 timestamp: {body}"
    );
}

fn id_of(value: &Value) -> i64 {
    value["id"].as_i64().expect("响应缺少 id")
}

#[tokio::test]
async fn anonymous_and_ordinary_are_blocked_with_contract_shapes() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "other", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let other_token = login(&env, "other").await;
    let admin_token = login(&env, "admin").await;

    // 匿名：9 条用户路由与 9 条管理员路由都是 401 安全入口 JSON。
    let user_routes: [(Method, &str, Option<Value>); 9] = [
        (Method::POST, "/api/markers", Some(create_body("x", "zh"))),
        (
            Method::PATCH,
            "/api/markers/1",
            Some(json!({ "title": "t" })),
        ),
        (Method::DELETE, "/api/markers/1", None),
        (Method::POST, "/api/markers/1/favorite", None),
        (Method::DELETE, "/api/markers/1/favorite", None),
        (Method::GET, "/api/markers/me/favorites", None),
        (Method::GET, "/api/markers/me/created", None),
        (Method::GET, "/api/markers/me/favorites/details", None),
        (Method::GET, "/api/markers/all", None),
    ];
    for (method, uri, body) in user_routes {
        let response = send(&env, method, uri, body, None, &[]).await;
        assert_security_entry(&response, uri);
    }

    let admin_routes: [(Method, &str); 9] = [
        (Method::GET, "/api/admin/markers/pending"),
        (Method::GET, "/api/admin/markers/pending-edits"),
        (Method::GET, "/api/admin/markers/all"),
        (Method::POST, "/api/admin/markers/1/approve"),
        (Method::POST, "/api/admin/markers/1/reject"),
        (Method::PATCH, "/api/admin/markers/1"),
        (Method::DELETE, "/api/admin/markers/1"),
        (Method::POST, "/api/admin/markers/edit-proposals/1/approve"),
        (Method::POST, "/api/admin/markers/edit-proposals/1/reject"),
    ];
    for (method, uri) in &admin_routes {
        let response = send(&env, method.clone(), uri, None, None, &[]).await;
        assert_security_entry(&response, uri);
    }

    // 普通用户：管理员路由是缺角色 Boot JSON。
    for (method, uri) in &admin_routes {
        let response = send(
            &env,
            method.clone(),
            uri,
            None,
            Some(other_token.as_str()),
            &[],
        )
        .await;
        assert_boot_forbidden(&response, uri);
    }
    let all_as_other = get(&env, "/api/markers/all", Some(other_token.as_str())).await;
    assert_boot_forbidden(&all_as_other, "/api/markers/all");

    // Logged-in admins can read both routes without secondary verification.
    let pending_unverified = get(
        &env,
        "/api/admin/markers/pending",
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(pending_unverified.status, StatusCode::OK);
    let all_unverified = get(&env, "/api/markers/all", Some(admin_token.as_str())).await;
    assert_eq!(all_unverified.status, StatusCode::OK);

    // 普通用户对自己的写路由可用（登录即可）。
    let created = post(
        &env,
        "/api/markers",
        create_body("登录创建", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    assert_marker_shape(&created.json());
}

#[tokio::test]
async fn create_review_favorite_list_and_admin_reads_lifecycle() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "other", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let other_token = login(&env, "other").await;
    let admin_token = login(&env, "admin").await;

    let created = post(
        &env,
        "/api/markers",
        create_body("新点位", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let marker = created.json();
    assert_marker_shape(&marker);
    assert_vary(created.headers());
    let id = id_of(&marker);
    assert_eq!(marker["reviewStatus"], "PENDING");
    assert_eq!(marker["isPublic"], json!(true));
    assert_eq!(marker["isActive"], json!(true));
    assert_eq!(marker["version"], json!(0));
    assert_eq!(marker["sourceLanguage"], "zh");
    assert_eq!(marker["contentLanguage"], "zh");
    assert_eq!(marker["username"], "owner");
    assert_eq!(marker["markImage"], Value::Null);
    assert_eq!(marker["clientRequestId"], Value::Null);

    // 详情资源级可见性：属主与管理员可见私有/待审，匿名与普通用户 404。
    let owner_detail = get(
        &env,
        &format!("/api/markers/{id}"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(owner_detail.status, StatusCode::OK);
    assert_marker_shape(&owner_detail.json());
    assert_vary(owner_detail.headers());
    assert_eq!(
        get(&env, &format!("/api/markers/{id}"), None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(
            &env,
            &format!("/api/markers/{id}"),
            Some(other_token.as_str())
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // 管理员不需要二次验证即可读到私有点位（资源可见性只要求当前角色）。
    assert_eq!(
        get(
            &env,
            &format!("/api/markers/{id}"),
            Some(admin_token.as_str())
        )
        .await
        .status,
        StatusCode::OK
    );
    let mine = get(&env, "/api/markers/me/created", Some(owner_token.as_str())).await;
    assert_eq!(mine.status, StatusCode::OK);
    assert_vary(mine.headers());
    let mine_json = mine.json();
    assert_eq!(mine_json.as_array().unwrap().len(), 1);
    assert_marker_shape(&mine_json[0]);
    assert_eq!(id_of(&mine_json[0]), id);
    let theirs = get(&env, "/api/markers/me/created", Some(other_token.as_str())).await;
    assert_eq!(theirs.json().as_array().unwrap().len(), 0);

    // 收藏不可见点位 → 404。
    let favorite_private = post_empty(
        &env,
        &format!("/api/markers/{id}/favorite"),
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(favorite_private.status, StatusCode::NOT_FOUND);

    // A normal admin login can approve directly, advancing the version once.
    let admin = admin_token.clone();
    let approved = post_empty(
        &env,
        &format!("/api/admin/markers/{id}/approve"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text());
    let approved_json = approved.json();
    assert_marker_shape(&approved_json);
    assert_vary(approved.headers());
    assert_eq!(approved_json["reviewStatus"], "APPROVED");
    assert_eq!(approved_json["version"], json!(1));

    // 通过后匿名可见（公开 + 已审核）。
    let anon = get(&env, &format!("/api/markers/{id}"), None).await;
    assert_eq!(anon.status, StatusCode::OK);
    assert_marker_shape(&anon.json());
    assert_vary(anon.headers());

    // 收藏幂等、ID 列表与详情本地化。
    for _ in 0..2 {
        let response = post_empty(
            &env,
            &format!("/api/markers/{id}/favorite"),
            Some(other_token.as_str()),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.body.is_empty(), "收藏成功应 200 空体");
    }
    let favorites = get(
        &env,
        "/api/markers/me/favorites",
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(favorites.status, StatusCode::OK);
    assert_eq!(favorites.json(), json!([id]));
    let favorite_details = get(
        &env,
        "/api/markers/me/favorites/details",
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(favorite_details.status, StatusCode::OK);
    assert_vary(favorite_details.headers());
    let details_json = favorite_details.json();
    assert_eq!(details_json.as_array().unwrap().len(), 1);
    assert_marker_shape(&details_json[0]);

    // 取消收藏幂等（不存在也成功）。
    for _ in 0..2 {
        let response = delete(
            &env,
            &format!("/api/markers/{id}/favorite"),
            Some(other_token.as_str()),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert!(response.body.is_empty());
    }
    assert_eq!(
        get(
            &env,
            "/api/markers/me/favorites",
            Some(other_token.as_str())
        )
        .await
        .json(),
        json!([])
    );

    // `/api/markers/all`：管理员不二次即可读，普通用户 Boot 403（已在前一用例覆盖）。
    let all = get(&env, "/api/markers/all", Some(admin_token.as_str())).await;
    assert_eq!(all.status, StatusCode::OK);
    assert_vary(all.headers());
    let all_json = all.json();
    assert_marker_shape(&all_json[0]);

    // 管理员 PATCH 直接编辑并推进版本。
    let updated = patch(
        &env,
        &format!("/api/admin/markers/{id}"),
        json!({ "title": "管理员改", "language": "zh" }),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.text());
    let updated_json = updated.json();
    assert_marker_shape(&updated_json);
    assert_vary(updated.headers());
    assert_eq!(updated_json["title"], "管理员改");
    assert_eq!(updated_json["version"], json!(2));
    assert_eq!(updated_json["reviewStatus"], "APPROVED");
}

#[tokio::test]
async fn owner_and_admin_delete_semantics() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "other", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let other_token = login(&env, "other").await;
    let admin = login_admin(&env, "admin").await;

    let created = post(
        &env,
        "/api/markers",
        create_body("待删", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    let id = id_of(&created.json());
    post_empty(
        &env,
        &format!("/api/admin/markers/{id}/approve"),
        Some(admin.as_str()),
    )
    .await;

    // 非属主删除 → 403 `无权限`（区别于其它 404）。
    let forbidden = delete(
        &env,
        &format!("/api/markers/{id}"),
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.text(), "无权限");

    // 不存在 → 404 `点位不存在`。
    let missing = delete(&env, "/api/markers/9999999", Some(owner_token.as_str())).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.text(), "点位不存在");

    // 属主删除 → 200 空体，随后不可见。
    let deleted = delete(
        &env,
        &format!("/api/markers/{id}"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert!(deleted.body.is_empty());
    assert_eq!(
        get(&env, &format!("/api/markers/{id}"), None).await.status,
        StatusCode::NOT_FOUND
    );

    // 管理员删除另一条：200 空体。
    let second = post(
        &env,
        "/api/markers",
        create_body("管理员删", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    let second_id = id_of(&second.json());
    let admin_deleted = delete(
        &env,
        &format!("/api/admin/markers/{second_id}"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(admin_deleted.status, StatusCode::OK);
    assert!(admin_deleted.body.is_empty());
    let admin_delete_missing = delete(
        &env,
        &format!("/api/admin/markers/{second_id}"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(admin_delete_missing.status, StatusCode::OK);
    let hidden = get(
        &env,
        &format!("/api/markers/{second_id}"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let all = get(&env, "/api/admin/markers/all", Some(admin.as_str())).await;
    assert!(
        all.json()
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == second_id && m["deactivated"] == true)
    );
    let forbidden_restore = post_empty(
        &env,
        &format!("/api/admin/markers/{second_id}/restore"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(forbidden_restore.status, StatusCode::FORBIDDEN);
    assert_eq!(
        post_empty(
            &env,
            &format!("/api/admin/markers/{second_id}/restore"),
            Some(admin.as_str())
        )
        .await
        .status,
        StatusCode::OK
    );
    // Restoring a pending point must not publish it or approve it.
    assert_eq!(
        get(&env, &format!("/api/markers/{second_id}"), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let owned = get(
        &env,
        &format!("/api/markers/{second_id}"),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(owned.json()["reviewStatus"], "PENDING");
    assert_eq!(owned.json()["deactivated"], false);
}

#[tokio::test]
async fn ordinary_patch_creates_pending_proposal_and_admin_review() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "other", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let other_token = login(&env, "other").await;
    let admin = login_admin(&env, "admin").await;

    let created = post(
        &env,
        "/api/markers",
        create_body("原点位", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    let id = id_of(&created.json());
    post_empty(
        &env,
        &format!("/api/admin/markers/{id}/approve"),
        Some(admin.as_str()),
    )
    .await;

    // 普通 PATCH 只建提案，返回未修改点位。
    let proposed = patch(
        &env,
        &format!("/api/markers/{id}"),
        json!({ "title": "他人提议", "language": "zh" }),
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(proposed.status, StatusCode::OK, "{}", proposed.text());
    let proposed_json = proposed.json();
    assert_marker_shape(&proposed_json);
    assert_vary(proposed.headers());
    assert_eq!(proposed_json["title"], "原点位");
    assert_eq!(proposed_json["version"], json!(1));

    // 待审提案列表字段与内容。
    let pending = get(
        &env,
        "/api/admin/markers/pending-edits",
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(pending.status, StatusCode::OK);
    let items = pending.json();
    assert_eq!(items.as_array().unwrap().len(), 1);
    assert_edit_proposal_shape(&items[0]);
    assert_eq!(items[0]["status"], "PENDING");
    assert_eq!(items[0]["markerId"], json!(id));
    assert_eq!(items[0]["title"], "他人提议");
    assert_eq!(items[0]["proposerUsername"], "other");
    assert_eq!(items[0]["proposerIsOwner"], json!(false));
    let proposal_id = id_of(&items[0]);
    let base: Option<i64> =
        sqlx::query_scalar("SELECT base_marker_version FROM marker_edit_proposals WHERE id = $1")
            .bind(proposal_id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(base, Some(1), "提案必须记录 base_marker_version");

    // 审核通过：应用文本、推进版本、状态 APPROVED。
    let approved = post_empty(
        &env,
        &format!("/api/admin/markers/edit-proposals/{proposal_id}/approve"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text());
    let approved_json = approved.json();
    assert_marker_shape(&approved_json);
    assert_vary(approved.headers());
    assert_eq!(approved_json["title"], "他人提议");
    assert_eq!(approved_json["version"], json!(2));
    assert_eq!(approved_json["lastEditedBy"], "other");
    assert_eq!(approved_json["lastEditedByOwner"], json!(false));

    // 重复审核 → 400 `该提案已处理`。
    let repeat = post_empty(
        &env,
        &format!("/api/admin/markers/edit-proposals/{proposal_id}/approve"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(repeat.status, StatusCode::BAD_REQUEST);
    assert_eq!(repeat.text(), "该提案已处理");

    // 第二条提案驳回：200 空体，点位不变。
    let second = patch(
        &env,
        &format!("/api/markers/{id}"),
        json!({ "title": "会被驳回", "language": "zh" }),
        Some(other_token.as_str()),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);
    let items = get(
        &env,
        "/api/admin/markers/pending-edits",
        Some(admin.as_str()),
    )
    .await
    .json();
    assert_eq!(items.as_array().unwrap().len(), 1);
    let second_proposal = id_of(&items[0]);
    let rejected = post_empty(
        &env,
        &format!("/api/admin/markers/edit-proposals/{second_proposal}/reject"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::OK);
    assert!(rejected.body.is_empty());
    let detail = get(&env, &format!("/api/markers/{id}"), None).await.json();
    assert_eq!(detail["title"], "他人提议");
    assert_eq!(detail["version"], json!(2));

    // 已处理提案再驳回 → 400；不存在提案 → 404。
    let reject_again = post_empty(
        &env,
        &format!("/api/admin/markers/edit-proposals/{second_proposal}/reject"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(reject_again.status, StatusCode::BAD_REQUEST);
    assert_eq!(reject_again.text(), "该提案已处理");
    let missing = post_empty(
        &env,
        "/api/admin/markers/edit-proposals/9999999/approve",
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.text(), "编辑提案不存在");
}

#[tokio::test]
async fn admin_pending_all_reject_and_translation_invalidation() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let admin = login_admin(&env, "admin").await;

    // 一条待审、一条已审核。
    let pending_id = id_of(
        &post(
            &env,
            "/api/markers",
            create_body("原文", "zh"),
            Some(owner_token.as_str()),
        )
        .await
        .json(),
    );
    let approved_id = id_of(
        &post(
            &env,
            "/api/markers",
            create_body("已审", "zh"),
            Some(owner_token.as_str()),
        )
        .await
        .json(),
    );
    post_empty(
        &env,
        &format!("/api/admin/markers/{approved_id}/approve"),
        Some(admin.as_str()),
    )
    .await;

    let pending = get(&env, "/api/admin/markers/pending", Some(admin.as_str())).await;
    assert_eq!(pending.status, StatusCode::OK);
    assert_vary(pending.headers());
    let pending_json = pending.json();
    assert_eq!(pending_json.as_array().unwrap().len(), 1);
    assert_marker_shape(&pending_json[0]);
    assert_eq!(id_of(&pending_json[0]), pending_id);
    assert_eq!(pending_json[0]["reviewStatus"], "PENDING");

    let all = get(&env, "/api/admin/markers/all", Some(admin.as_str())).await;
    assert_eq!(all.status, StatusCode::OK);
    let all_json = all.json();
    assert_eq!(all_json.as_array().unwrap().len(), 2);

    // 直接 reject 推进版本。
    let rejected = post_empty(
        &env,
        &format!("/api/admin/markers/{pending_id}/reject"),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(rejected.status, StatusCode::OK, "{}", rejected.text());
    let rejected_json = rejected.json();
    assert_marker_shape(&rejected_json);
    assert_eq!(rejected_json["reviewStatus"], "REJECTED");
    assert_eq!(rejected_json["version"], json!(1));
    assert_eq!(
        get(&env, "/api/admin/markers/pending", Some(admin.as_str()))
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // 管理员写译文：原文不变；`?lang=en` 读到译文，`?lang=zh` 读原文。
    let translated = patch(
        &env,
        &format!("/api/admin/markers/{approved_id}"),
        json!({
            "language": "en",
            "title": "English title",
            "description": "English desc",
        }),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(translated.status, StatusCode::OK, "{}", translated.text());
    let translated_json = translated.json();
    assert_eq!(translated_json["title"], "已审", "原文不得被译文覆盖");
    assert_eq!(translated_json["version"], json!(2));

    let en = get(&env, &format!("/api/markers/{approved_id}?lang=en"), None).await;
    assert_eq!(en.status, StatusCode::OK);
    assert_vary(en.headers());
    let en_json = en.json();
    assert_eq!(en_json["title"], "English title");
    assert_eq!(en_json["description"], "English desc");
    assert_eq!(en_json["contentLanguage"], "en");

    let zh = get(&env, &format!("/api/markers/{approved_id}?lang=zh"), None).await;
    assert_eq!(zh.json()["title"], "已审");
    assert_eq!(zh.json()["contentLanguage"], "zh");

    // 原文变更令旧译文失效：回退原文且 contentLanguage=zh；译文行仍在。
    let invalidated = patch(
        &env,
        &format!("/api/admin/markers/{approved_id}"),
        json!({ "language": "zh", "title": "新原文", "description": "新描述" }),
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(invalidated.status, StatusCode::OK);
    assert_eq!(invalidated.json()["version"], json!(3));

    let stale = get(&env, &format!("/api/markers/{approved_id}?lang=en"), None).await;
    assert_eq!(stale.json()["title"], "新原文", "过期译文必须回退原文");
    assert_eq!(stale.json()["contentLanguage"], "zh");
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM map_marker_translations WHERE marker_id = $1 AND language = 'en'",
    )
    .bind(approved_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(origin, "MANUAL", "失效译文不得删除或改 origin");
}

#[tokio::test]
async fn concurrent_edit_proposal_review_allows_one_and_conflicts_other() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "admin1", "ADMIN").await;
    let _ = insert_user(&env, "admin2", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let admin1 = login_admin(&env, "admin1").await;
    let admin2 = login_admin(&env, "admin2").await;

    let created = post(
        &env,
        "/api/markers",
        create_body("基准", "zh"),
        Some(owner_token.as_str()),
    )
    .await;
    let id = id_of(&created.json());
    post_empty(
        &env,
        &format!("/api/admin/markers/{id}/approve"),
        Some(admin1.as_str()),
    )
    .await;

    // 同一基准版本下：一个原文提案、一个译文提案。
    let first = patch(
        &env,
        &format!("/api/markers/{id}"),
        json!({ "title": "源提案", "language": "zh" }),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = patch(
        &env,
        &format!("/api/markers/{id}"),
        json!({
            "title": "EN 提案",
            "description": "EN desc",
            "language": "en",
        }),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);

    let proposals: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM marker_edit_proposals WHERE marker_id = $1 ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(proposals.len(), 2);

    let uri_first = format!("/api/admin/markers/edit-proposals/{}/approve", proposals[0]);
    let uri_second = format!("/api/admin/markers/edit-proposals/{}/approve", proposals[1]);
    let (left, right) = tokio::join!(
        send(
            &env,
            Method::POST,
            &uri_first,
            None,
            Some(admin1.as_str()),
            &[]
        ),
        send(
            &env,
            Method::POST,
            &uri_second,
            None,
            Some(admin2.as_str()),
            &[],
        ),
    );

    let successes = [left.status, right.status]
        .into_iter()
        .filter(|status| *status == StatusCode::OK)
        .count();
    assert_eq!(successes, 1, "同基准两提案并发审核只允许一人成功");
    let loser = if left.status == StatusCode::OK {
        &right
    } else {
        &left
    };
    assert_eq!(loser.status, StatusCode::CONFLICT);
    assert_eq!(loser.text(), MSG_STALE_VERSION);

    let version: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id = $1")
        .bind(id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(version, 2, "只应推进一次版本");
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marker_edit_proposals WHERE marker_id = $1 AND status = 'PENDING'",
    )
    .bind(id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(pending, 1, "输家提案必须仍待审");
}

#[tokio::test]
async fn create_mark_image_only_null_or_blank() {
    let env = Env::new().await;
    let owner_id = insert_user(&env, "owner", "USER").await;
    let owner_token = login(&env, "owner").await;

    // 非空私有 URL → 400，且不落点。
    let mut suspicious = create_body("可疑引用", "zh");
    suspicious["markImage"] = json!("/uploads/markers/private.png");
    let rejected = post(&env, "/api/markers", suspicious, Some(owner_token.as_str())).await;
    assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected.text(), MSG_MARK_IMAGE_UPLOAD_ONLY);
    let mine = get(&env, "/api/markers/me/created", Some(owner_token.as_str())).await;
    assert!(
        mine.json().as_array().unwrap().is_empty(),
        "非空 markImage 不得创建点位"
    );
    let suspicious_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM map_markers WHERE title = $1")
            .bind("可疑引用")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(suspicious_rows, 0, "非空 markImage 请求不得落点");

    // null / 空串 / 空白串都接受并落为 null。
    for (title, mark_image) in [
        ("显式 null", Value::Null),
        ("空串", json!("")),
        ("空白串", json!("   ")),
    ] {
        let mut body = create_body(title, "zh");
        body["markImage"] = mark_image;
        let created = post(&env, "/api/markers", body, Some(owner_token.as_str())).await;
        assert_eq!(
            created.status,
            StatusCode::OK,
            "{title}: {}",
            created.text()
        );
        assert_eq!(
            created.json()["markImage"],
            Value::Null,
            "{title} 应落为 null"
        );
    }

    // 幂等重放：命中已存在 key 时先返回原点位，不采纳重放里的非空 markImage。
    let mut original = create_body("幂等原点位", "zh");
    original["clientRequestId"] = json!("replay-key");
    let first = post(&env, "/api/markers", original, Some(owner_token.as_str())).await;
    assert_eq!(first.status, StatusCode::OK);
    let first_json = first.json();
    let original_id = id_of(&first_json);
    assert_eq!(first_json["markImage"], Value::Null);

    let mut replay = create_body("完全不同的载荷", "zh");
    replay["clientRequestId"] = json!("replay-key");
    replay["markImage"] = json!("/uploads/markers/private.png");
    let replayed = post(&env, "/api/markers", replay, Some(owner_token.as_str())).await;
    assert_eq!(replayed.status, StatusCode::OK);
    let replayed_json = replayed.json();
    assert_eq!(id_of(&replayed_json), original_id);
    assert_eq!(replayed_json["title"], "幂等原点位");
    assert_eq!(replayed_json["markImage"], Value::Null);

    // 核心层（非仅 handler）规则：非空恶意 URL 在首次完整校验被拒；null/空串/空白归一为 None。
    let actor = Actor::new(owner_id.to_string(), "owner", false);
    let service_request = |title: &str, mark_image: Option<String>| MarkerCreateRequest {
        lat: Some(1.0),
        lng: Some(2.0),
        category: Some("accessible_toilet".to_string()),
        title: Some(title.to_string()),
        description: None,
        language: Some("zh".to_string()),
        is_public: None,
        is_active: None,
        open_time_start: None,
        open_time_end: None,
        client_request_id: None,
        mark_image,
        venue_type: None,
    };

    let malicious = service_request("核心拒绝", Some("/uploads/markers/private.png".to_string()));
    let error = env
        .state
        .markers_write
        .create_marker(&actor, "zh", malicious)
        .await
        .unwrap_err();
    assert!(
        matches!(error, WriteError::BadRequest(ref message) if message == MSG_MARK_IMAGE_UPLOAD_ONLY),
        "核心首次校验必须拒绝非空 markImage，实际 {error:?}"
    );
    let core_rejected: i64 =
        sqlx::query_scalar("SELECT count(*) FROM map_markers WHERE title = $1")
            .bind("核心拒绝")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(core_rejected, 0, "核心拒绝后不得落点");

    for (title, mark_image) in [
        ("核心 null", None),
        ("核心空串", Some(String::new())),
        ("核心空白", Some("   ".to_string())),
    ] {
        let row = env
            .state
            .markers_write
            .create_marker(&actor, "zh", service_request(title, mark_image))
            .await
            .unwrap_or_else(|error| panic!("{title} 服务层应接受，实际 {error:?}"));
        assert_eq!(row.mark_image, None, "{title} 应归一为 None");
    }
}

#[tokio::test]
async fn marker_write_json_body_uses_global_limit_with_contract_errors() {
    let env = Env::new().await;
    let _ = insert_user(&env, "owner", "USER").await;
    let _ = insert_user(&env, "admin", "ADMIN").await;
    let owner_token = login(&env, "owner").await;
    let admin = login_admin(&env, "admin").await;

    // 超过认证 64 KiB、但小于全局 8 MiB 的合法 description：点位创建必须成功。
    let big = "d".repeat(70 * 1024);
    let mut create = create_body("大文本", "zh");
    create["description"] = json!(big);
    let created = post(&env, "/api/markers", create, Some(owner_token.as_str())).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let created_json = created.json();
    assert_marker_shape(&created_json);
    assert_eq!(
        created_json["description"].as_str().unwrap().len(),
        70 * 1024,
        "大 description 不得被 64 KiB 认证上限截断或拒绝"
    );
    let id = id_of(&created_json);

    // 普通 PATCH 与管理员 PATCH 同样使用全局大上限。
    let patch_big = json!({ "language": "zh", "description": "p".repeat(70 * 1024) });
    let proposed = patch(
        &env,
        &format!("/api/markers/{id}"),
        patch_big.clone(),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(proposed.status, StatusCode::OK, "{}", proposed.text());

    let admin_patched = patch(
        &env,
        &format!("/api/admin/markers/{id}"),
        patch_big,
        Some(admin.as_str()),
    )
    .await;
    assert_eq!(
        admin_patched.status,
        StatusCode::OK,
        "{}",
        admin_patched.text()
    );
    assert_marker_shape(&admin_patched.json());

    // 非法 JSON → 400 中文纯文本（不是 422，也不假称超限）。
    let invalid = send_raw(
        &env,
        Method::POST,
        "/api/markers",
        "application/json",
        b"{ not json".to_vec(),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.text(), "请求参数不合法");
    assert!(
        !invalid
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .contains("json"),
        "解析错误应为纯文本"
    );

    // 不支持的媒体类型 → 415 中文纯文本。
    let unsupported = send_raw(
        &env,
        Method::POST,
        "/api/markers",
        "text/plain",
        serde_json::to_vec(&create_body("类型", "zh")).unwrap(),
        Some(owner_token.as_str()),
    )
    .await;
    assert_eq!(unsupported.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(unsupported.text(), "请求内容类型必须是 application/json");

    // 超过全局 8 MiB → 413 统一 ApiResponse 上传上限文案。
    let mut oversized = create_body("超大", "zh");
    oversized["description"] = json!("d".repeat(REQUEST_BODY_LIMIT_BYTES + 1024));
    let rejected = post(&env, "/api/markers", oversized, Some(owner_token.as_str())).await;
    assert_eq!(rejected.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        rejected.json(),
        json!({
            "code": 413,
            "message": "上传文件过大，请选择 5MB 以内的图片",
            "data": Value::Null,
        }),
        "超限应为统一 ApiResponse 上传上限文案"
    );
}
