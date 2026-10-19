//! 管理后台 v2：审核理由、事件、批量、搜索、提交记录与统计的真实 PG / Redis 集成测试。

mod common;

use std::net::IpAddr;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use common::{TempDatabase, connect_redis};
use fred::clients::Client;
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::password::PasswordHasher;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const COOKIE_NAME: &str = "LYCORIS_SESSION";
const PASSWORD: &str = "test-password";

struct Env {
    _temp: TempDatabase,
    _upload: tempfile::TempDir,
    pool: PgPool,
    router: Router,
    _redis: Client,
}

impl Env {
    async fn new() -> Self {
        let (temp, pool) = TempDatabase::create_migrated().await;
        let redis = connect_redis().await;
        let unique = Uuid::new_v4().simple().to_string();
        let mut config = Config::new(temp.url(), "redis://127.0.0.1:56379");
        config.session_namespace = format!("lycoris:test:{unique}:session");
        config.rate_limit_namespace = format!("lycoris:test:{unique}:ratelimit");
        config.marker_cache_namespace = format!("lycoris:test:{unique}:markers");
        config.bcrypt_cost = 4;
        let upload = tempfile::TempDir::new().unwrap();
        config.upload_dir = upload.path().to_path_buf();
        let state = AppState::new(pool.clone(), redis.clone(), config).unwrap();
        let router = build_router(state);
        Self {
            _temp: temp,
            _upload: upload,
            pool,
            router,
            _redis: redis,
        }
    }

    async fn user(&self, username: &str, role: &str) -> (i32, String, String) {
        let hash = PasswordHasher::new(4, 1)
            .hash(PASSWORD.to_string())
            .await
            .unwrap();
        let public_id = Uuid::new_v4();
        let id: i32 = sqlx::query_scalar(
            "INSERT INTO users (public_id, username, nickname, email, password, role, deleted,
                session_version, row_version)
             VALUES ($1,$2,$2,$3,$4,$5,false,0,0) RETURNING id",
        )
        .bind(public_id)
        .bind(username)
        .bind(format!("{username}@example.com"))
        .bind(&hash)
        .bind(role)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        let cookie = self.login(username).await;
        (id, public_id.to_string(), cookie)
    }

    async fn login(&self, username: &str) -> String {
        let response = self
            .call(
                Method::POST,
                "/api/login",
                None,
                Some(json!({ "username": username, "password": PASSWORD })),
            )
            .await;
        assert_eq!(response.status, StatusCode::OK);
        response
            .cookie(COOKIE_NAME)
            .expect("登录响应应设置会话 Cookie")
    }

    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, format!("{COOKIE_NAME}={cookie}"));
        }
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&value).unwrap())
            }
            None => Body::empty(),
        };
        let mut request = builder.body(body).unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
                IpAddr::from([127, 0, 0, 1]),
                40000,
            )));
        let response = self.router.clone().oneshot(request).await.unwrap();
        Resp::from(response).await
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
        let body = to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec();
        Self {
            status,
            headers,
            body,
        }
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn cookie(&self, name: &str) -> Option<String> {
        for value in self.headers.get_all(header::SET_COOKIE) {
            let raw = value.to_str().ok()?;
            let first = raw.split(';').next().unwrap_or_default();
            if let Some(rest) = first.strip_prefix(&format!("{name}=")) {
                return Some(rest.to_string());
            }
        }
        None
    }
}

async fn seed_marker(
    pool: &PgPool,
    title: &str,
    status: &str,
    owner_public_id: &str,
    username: &str,
    category: &str,
    deactivated: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            review_status, username, user_public_id, categories, deactivated,
            last_edited_by_owner, created_at, updated_at)
         VALUES (1.0, 2.0, $1, $2, NULL, 'zh', true, true, $3, $4, $5, ARRAY[$1], $6, false, now(), now())
         RETURNING id",
    )
    .bind(category)
    .bind(title)
    .bind(status)
    .bind(username)
    .bind(owner_public_id)
    .bind(deactivated)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn seed_edit(
    pool: &PgPool,
    marker_id: i64,
    marker_title: &str,
    title: &str,
    proposer_public_id: &str,
    proposer_username: &str,
    base_version: i64,
    status: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO marker_edit_proposals (
            marker_id, marker_title, marker_lat, marker_lng, proposer_username,
            proposer_public_id, proposer_is_owner, category, title, description, language,
            is_public, is_active, base_marker_version, categories, status, created_at)
         VALUES ($1,$2,1.0,2.0,$3,$4,false,'accessible_toilet',$5,NULL,'zh',true,true,$6,
                 ARRAY['accessible_toilet'],$7,now())
         RETURNING id",
    )
    .bind(marker_id)
    .bind(marker_title)
    .bind(proposer_username)
    .bind(proposer_public_id)
    .bind(title)
    .bind(base_version)
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_image(
    pool: &PgPool,
    marker_id: i64,
    marker_title: &str,
    proposer_public_id: &str,
    proposer_username: &str,
    image_url: &str,
    status: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO marker_image_proposals (
            marker_id, marker_title, proposer_username, proposer_public_id, image_url, status, created_at)
         VALUES ($1,$2,$3,$4,$5,$6,now()) RETURNING id",
    )
    .bind(marker_id)
    .bind(marker_title)
    .bind(proposer_username)
    .bind(proposer_public_id)
    .bind(image_url)
    .bind(status)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn review_note(pool: &PgPool, table: &str, id: i64) -> Option<String> {
    let sql = format!("SELECT review_note FROM {table} WHERE id = $1");
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn marker_status(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar("SELECT review_status FROM map_markers WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn event_rows(pool: &PgPool) -> Vec<(String, String, i64, Option<i64>, Option<String>)> {
    sqlx::query_as(
        "SELECT action, target_kind, target_id, marker_id, note FROM moderation_events
         ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn block_on_settle() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
async fn legacy_review_calls_without_body_keep_working() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("legacy-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("legacy-owner", "USER").await;

    let approve_marker = seed_marker(
        &env.pool,
        "approve",
        "PENDING",
        &owner,
        "legacy-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let reject_marker = seed_marker(
        &env.pool,
        "reject",
        "PENDING",
        &owner,
        "legacy-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit_marker = seed_marker(
        &env.pool,
        "edit",
        "APPROVED",
        &owner,
        "legacy-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let approve_edit = seed_edit(
        &env.pool,
        edit_marker,
        "edit",
        "edit-approve",
        &owner,
        "legacy-owner",
        0,
        "PENDING",
    )
    .await;
    let reject_edit = seed_edit(
        &env.pool,
        edit_marker,
        "edit",
        "edit-reject",
        &owner,
        "legacy-owner",
        0,
        "PENDING",
    )
    .await;
    let image_marker = seed_marker(
        &env.pool,
        "image",
        "APPROVED",
        &owner,
        "legacy-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let approve_image = seed_image(
        &env.pool,
        image_marker,
        "image",
        &owner,
        "legacy-owner",
        "/uploads/markers/a.png",
        "PENDING",
    )
    .await;
    let reject_image = seed_image(
        &env.pool,
        image_marker,
        "image",
        &owner,
        "legacy-owner",
        "/uploads/markers/b.png",
        "PENDING",
    )
    .await;

    for (method, path) in [
        (
            Method::POST,
            format!("/api/admin/markers/{approve_marker}/approve"),
        ),
        (
            Method::POST,
            format!("/api/admin/markers/{reject_marker}/reject"),
        ),
        (
            Method::POST,
            format!("/api/admin/markers/edit-proposals/{approve_edit}/approve"),
        ),
        (
            Method::POST,
            format!("/api/admin/markers/edit-proposals/{reject_edit}/reject"),
        ),
        (
            Method::POST,
            format!("/api/admin/markers/image-proposals/{approve_image}/approve"),
        ),
        (
            Method::POST,
            format!("/api/admin/markers/image-proposals/{reject_image}/reject"),
        ),
    ] {
        let response = env.call(method, &path, Some(&admin), None).await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{path}: {}",
            response.text()
        );
    }
    assert_eq!(marker_status(&env.pool, approve_marker).await, "APPROVED");
    assert_eq!(marker_status(&env.pool, reject_marker).await, "REJECTED");
}

#[tokio::test]
async fn reject_reasons_are_saved_trimmed_limited_and_cleared_on_approve() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("reason-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("reason-owner", "USER").await;

    let marker = seed_marker(
        &env.pool,
        "reason",
        "PENDING",
        &owner,
        "reason-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let rejected = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{marker}/reject"),
            Some(&admin),
            Some(json!({ "reason": "  不符合规范  " })),
        )
        .await;
    assert_eq!(rejected.status, StatusCode::OK, "{}", rejected.text());
    assert_eq!(
        review_note(&env.pool, "map_markers", marker)
            .await
            .as_deref(),
        Some("不符合规范")
    );
    let events = event_rows(&env.pool).await;
    assert_eq!(events.last().unwrap().0, "reject");
    assert_eq!(events.last().unwrap().4.as_deref(), Some("不符合规范"));

    let too_long = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{marker}/reject"),
            Some(&admin),
            Some(json!({ "reason": "字".repeat(501) })),
        )
        .await;
    assert_eq!(too_long.status, StatusCode::BAD_REQUEST);
    assert_eq!(too_long.text(), "理由过长");

    let approved = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{marker}/approve"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(approved.status, StatusCode::OK);
    assert_eq!(review_note(&env.pool, "map_markers", marker).await, None);

    let deactivated = seed_marker(
        &env.pool,
        "deactivated",
        "APPROVED",
        &owner,
        "reason-owner",
        "accessible_toilet",
        true,
    )
    .await;
    sqlx::query("UPDATE map_markers SET review_note = '恢复后清空' WHERE id = $1")
        .bind(deactivated)
        .execute(&env.pool)
        .await
        .unwrap();
    let restored = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{deactivated}/restore"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(restored.status, StatusCode::OK);
    assert_eq!(
        review_note(&env.pool, "map_markers", deactivated).await,
        None
    );

    let edit_marker = seed_marker(
        &env.pool,
        "edit",
        "APPROVED",
        &owner,
        "reason-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit = seed_edit(
        &env.pool,
        edit_marker,
        "edit",
        "edit",
        &owner,
        "reason-owner",
        0,
        "PENDING",
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/markers/edit-proposals/{edit}/reject"),
        Some(&admin),
        Some(json!({ "reason": "驳回编辑" })),
    )
    .await;
    assert_eq!(
        review_note(&env.pool, "marker_edit_proposals", edit)
            .await
            .as_deref(),
        Some("驳回编辑")
    );
    let image_marker = seed_marker(
        &env.pool,
        "image",
        "APPROVED",
        &owner,
        "reason-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let image = seed_image(
        &env.pool,
        image_marker,
        "image",
        &owner,
        "reason-owner",
        "/uploads/markers/c.png",
        "PENDING",
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/markers/image-proposals/{image}/reject"),
        Some(&admin),
        Some(json!({ "reason": "图片模糊" })),
    )
    .await;
    assert_eq!(
        review_note(&env.pool, "marker_image_proposals", image)
            .await
            .as_deref(),
        Some("图片模糊")
    );
}

#[tokio::test]
async fn approve_with_changes_updates_before_approving_and_rejects_invalid_changes_atomically() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("changes-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("changes-owner", "USER").await;

    let marker = seed_marker(
        &env.pool,
        "原题",
        "PENDING",
        &owner,
        "changes-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let approved = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{marker}/approve"),
            Some(&admin),
            Some(json!({ "changes": { "title": "修改后", "language": "zh" } })),
        )
        .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text());
    assert_eq!(approved.json()["title"], "修改后");
    assert_eq!(approved.json()["reviewStatus"], "APPROVED");
    let events = event_rows(&env.pool).await;
    assert_eq!(events.last().unwrap().0, "approve_with_changes");

    let edit_marker = seed_marker(
        &env.pool,
        "原点位",
        "APPROVED",
        &owner,
        "changes-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit = seed_edit(
        &env.pool,
        edit_marker,
        "原点位",
        "提案标题",
        &owner,
        "changes-owner",
        0,
        "PENDING",
    )
    .await;
    let approved_edit = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/edit-proposals/{edit}/approve"),
            Some(&admin),
            Some(json!({ "changes": { "title": "最终标题", "language": "zh" } })),
        )
        .await;
    assert_eq!(
        approved_edit.status,
        StatusCode::OK,
        "{}",
        approved_edit.text()
    );
    assert_eq!(approved_edit.json()["title"], "最终标题");
    assert_eq!(approved_edit.json()["lastEditedBy"], "changes-owner");
    assert_eq!(
        event_rows(&env.pool).await.last().unwrap().0,
        "approve_with_changes"
    );

    let pending = seed_marker(
        &env.pool,
        "invalid",
        "PENDING",
        &owner,
        "changes-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let before = event_rows(&env.pool).await.len();
    let invalid = env
        .call(
            Method::POST,
            &format!("/api/admin/markers/{pending}/approve"),
            Some(&admin),
            Some(json!({ "changes": { "title": "   " } })),
        )
        .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    assert_eq!(invalid.text(), "title 不能为空");
    assert_eq!(marker_status(&env.pool, pending).await, "PENDING");
    assert_eq!(
        event_rows(&env.pool).await.len(),
        before,
        "校验失败不得留下事件"
    );
    block_on_settle().await;
}

#[tokio::test]
async fn batch_allows_partial_success_and_validates_shape() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("batch-admin", "ADMIN").await;
    let (_owner_id, owner, owner_cookie) = env.user("batch-owner", "USER").await;

    let first = seed_marker(
        &env.pool,
        "first",
        "PENDING",
        &owner,
        "batch-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let second = seed_marker(
        &env.pool,
        "second",
        "PENDING",
        &owner,
        "batch-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit_marker = seed_marker(
        &env.pool,
        "edit",
        "APPROVED",
        &owner,
        "batch-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit = seed_edit(
        &env.pool,
        edit_marker,
        "edit",
        "batch edit",
        &owner,
        "batch-owner",
        0,
        "PENDING",
    )
    .await;

    let ok = env
        .call(
            Method::POST,
            "/api/admin/moderation/batch",
            Some(&admin),
            Some(json!({ "items": [
                { "kind": "marker", "id": first, "decision": "approve" },
                { "kind": "marker", "id": second, "decision": "reject", "reason": "重复" },
                { "kind": "edit", "id": edit, "decision": "approve" }
            ]})),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    let results = ok.json()["results"].as_array().unwrap().clone();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|result| result["ok"] == true));
    assert_eq!(marker_status(&env.pool, first).await, "APPROVED");
    assert_eq!(marker_status(&env.pool, second).await, "REJECTED");

    // 让编辑提案的基准版本过期：同批次应部分成功。
    sqlx::query("UPDATE map_markers SET version = version + 1 WHERE id = $1")
        .bind(edit_marker)
        .execute(&env.pool)
        .await
        .unwrap();
    let stale = seed_edit(
        &env.pool,
        edit_marker,
        "edit",
        "stale",
        &owner,
        "batch-owner",
        0,
        "PENDING",
    )
    .await;
    let third = seed_marker(
        &env.pool,
        "third",
        "PENDING",
        &owner,
        "batch-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let partial = env
        .call(
            Method::POST,
            "/api/admin/moderation/batch",
            Some(&admin),
            Some(json!({ "items": [
                { "kind": "edit", "id": stale, "decision": "approve" },
                { "kind": "marker", "id": third, "decision": "approve" }
            ]})),
        )
        .await;
    assert_eq!(partial.status, StatusCode::OK);
    let partial = partial.json()["results"].as_array().unwrap().clone();
    assert_eq!(partial[0]["ok"], false);
    assert_eq!(partial[0]["status"], 409);
    assert!(
        partial[0]["message"]
            .as_str()
            .unwrap()
            .contains("点位已更新")
    );
    assert_eq!(partial[1]["ok"], true);
    let stale_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM moderation_events WHERE target_kind = 'edit' AND target_id = $1",
    )
    .bind(stale)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(stale_events, 0, "409 失败不得留下事件");

    let too_many: Vec<Value> = (0..51)
        .map(|id| json!({ "kind": "marker", "id": id, "decision": "approve" }))
        .collect();
    let too_many = env
        .call(
            Method::POST,
            "/api/admin/moderation/batch",
            Some(&admin),
            Some(json!({ "items": too_many })),
        )
        .await;
    assert_eq!(too_many.status, StatusCode::BAD_REQUEST);

    let duplicate = env
        .call(
            Method::POST,
            "/api/admin/moderation/batch",
            Some(&admin),
            Some(json!({ "items": [
                { "kind": "marker", "id": third, "decision": "approve" },
                { "kind": "marker", "id": third, "decision": "reject" }
            ]})),
        )
        .await;
    assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);

    let forbidden = env
        .call(
            Method::POST,
            "/api/admin/moderation/batch",
            Some(&owner_cookie),
            Some(json!({ "items": [{ "kind": "marker", "id": third, "decision": "approve" }] })),
        )
        .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_search_paginates_filters_sorts_and_escapes_wildcards() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("search-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("search-owner", "USER").await;

    let percent = seed_marker(
        &env.pool,
        "100% place",
        "APPROVED",
        &owner,
        "alice",
        "accessible_toilet",
        false,
    )
    .await;
    let underscore = seed_marker(
        &env.pool,
        "a_b place",
        "REJECTED",
        &owner,
        "alice",
        "accessible_toilet",
        true,
    )
    .await;
    let plain = seed_marker(
        &env.pool,
        "100x place",
        "APPROVED",
        &owner,
        "bob",
        "baby_room",
        false,
    )
    .await;
    let pending = seed_marker(
        &env.pool,
        "axb place",
        "PENDING",
        &owner,
        "alice",
        "self_definition",
        false,
    )
    .await;

    sqlx::query("UPDATE map_markers SET review_note = '旧理由' WHERE id = $1")
        .bind(underscore)
        .execute(&env.pool)
        .await
        .unwrap();
    // 固定创建时间以验证排序。
    sqlx::query("UPDATE map_markers SET created_at = now() - interval '4 days' WHERE id = $1")
        .bind(underscore)
        .execute(&env.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE map_markers SET created_at = now() - interval '5 days' WHERE id = $1")
        .bind(percent)
        .execute(&env.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE map_markers SET created_at = now() - interval '2 days' WHERE id = $1")
        .bind(plain)
        .execute(&env.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE map_markers SET created_at = now() - interval '1 day' WHERE id = $1")
        .bind(pending)
        .execute(&env.pool)
        .await
        .unwrap();

    let page = env
        .call(
            Method::GET,
            "/api/admin/markers/search?page=0&size=2",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    let body = page.json();
    assert_eq!(body["page"], 0);
    assert_eq!(body["size"], 2);
    assert_eq!(body["totalElements"], 4);
    assert_eq!(body["totalPages"], 2);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert!(body["items"][0].get("reviewNote").is_some());

    let second = env
        .call(
            Method::GET,
            "/api/admin/markers/search?page=1&size=2",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(second["page"], 1);
    assert_eq!(second["items"].as_array().unwrap().len(), 2);

    let wildcard = env
        .call(
            Method::GET,
            "/api/admin/markers/search?q=100%25",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(wildcard["totalElements"], 1);
    assert_eq!(wildcard["items"][0]["id"], percent);

    let underscore_query = env
        .call(
            Method::GET,
            "/api/admin/markers/search?q=a_b",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(underscore_query["totalElements"], 1);
    assert_eq!(underscore_query["items"][0]["id"], underscore);
    assert_eq!(underscore_query["items"][0]["reviewNote"], "旧理由");

    let dxb = env
        .call(
            Method::GET,
            "/api/admin/markers/search?q=axb",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(dxb["totalElements"], 1);
    assert_eq!(dxb["items"][0]["id"], pending);

    let numeric = env
        .call(
            Method::GET,
            &format!("/api/admin/markers/search?q={plain}"),
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(numeric["totalElements"], 1);
    assert_eq!(numeric["items"][0]["id"], plain);

    let rejected = env
        .call(
            Method::GET,
            "/api/admin/markers/search?status=REJECTED",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(rejected["totalElements"], 1);
    assert_eq!(rejected["items"][0]["id"], underscore);

    let category = env
        .call(
            Method::GET,
            "/api/admin/markers/search?category=baby_room",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(category["totalElements"], 1);
    assert_eq!(category["items"][0]["id"], plain);

    let deactivated = env
        .call(
            Method::GET,
            "/api/admin/markers/search?deactivated=true",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(deactivated["totalElements"], 1);
    assert_eq!(deactivated["items"][0]["id"], underscore);

    let active = env
        .call(
            Method::GET,
            "/api/admin/markers/search?deactivated=false",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(active["totalElements"], 3);

    let oldest = env
        .call(
            Method::GET,
            "/api/admin/markers/search?sort=created_asc",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(oldest["items"][0]["id"], percent);
    let newest = env
        .call(
            Method::GET,
            "/api/admin/markers/search?sort=created_desc",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(newest["items"][0]["id"], pending);
}

#[tokio::test]
async fn moderation_events_filter_paginate_and_record_user_operations() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("events-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("events-owner", "USER").await;
    let (target_id, _target_public, _) = env.user("events-target", "USER").await;

    let first = seed_marker(
        &env.pool,
        "one",
        "PENDING",
        &owner,
        "events-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let second = seed_marker(
        &env.pool,
        "two",
        "PENDING",
        &owner,
        "events-owner",
        "accessible_toilet",
        false,
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/markers/{first}/approve"),
        Some(&admin),
        None,
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/markers/{second}/reject"),
        Some(&admin),
        None,
    )
    .await;

    let filtered = env
        .call(
            Method::GET,
            &format!("/api/admin/moderation/events?markerId={first}"),
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(filtered["totalElements"], 1);
    assert_eq!(filtered["items"][0]["markerId"], first);
    assert_eq!(filtered["items"][0]["action"], "approve");
    assert_eq!(filtered["items"][0]["targetKind"], "marker");
    assert_eq!(filtered["items"][0]["markerTitle"], "one");
    assert!(filtered["items"][0]["targetLabel"].is_null());

    let paged = env
        .call(
            Method::GET,
            "/api/admin/moderation/events?page=0&size=1",
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(paged["size"], 1);
    assert_eq!(paged["items"].as_array().unwrap().len(), 1);
    assert_eq!(paged["totalElements"], 2);
    assert_eq!(paged["totalPages"], 2);

    env.call(
        Method::DELETE,
        &format!("/api/admin/users/{target_id}"),
        Some(&admin),
        None,
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/users/{target_id}/restore"),
        Some(&admin),
        None,
    )
    .await;
    env.call(
        Method::POST,
        &format!("/api/admin/users/{target_id}/reset-password"),
        Some(&admin),
        None,
    )
    .await;

    let all = env
        .call(
            Method::GET,
            "/api/admin/moderation/events?size=100",
            Some(&admin),
            None,
        )
        .await
        .json();
    let actions: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["action"].as_str().unwrap())
        .collect();
    for expected in ["user_disable", "user_restore", "user_reset_password"] {
        assert!(actions.contains(&expected), "缺少 {expected}: {actions:?}");
    }
    let user_items: Vec<&Value> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["targetKind"] == "user")
        .collect();
    assert!(!user_items.is_empty());
    assert!(user_items.iter().all(|item| item["markerId"].is_null()));
    assert!(user_items.iter().all(|item| item["markerTitle"].is_null()));
    assert!(
        user_items
            .iter()
            .all(|item| item["targetLabel"] == "events-target")
    );
}

#[tokio::test]
async fn pending_edits_expose_base_marker_version_for_stale_detection() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("stale-admin", "ADMIN").await;
    let (_owner_id, owner, _) = env.user("stale-owner", "USER").await;

    let marker = seed_marker(
        &env.pool,
        "会过期的点位",
        "APPROVED",
        &owner,
        "stale-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let proposal = seed_edit(
        &env.pool,
        marker,
        "会过期的点位",
        "过期提案",
        &owner,
        "stale-owner",
        0,
        "PENDING",
    )
    .await;
    sqlx::query("UPDATE map_markers SET version = 5 WHERE id = $1")
        .bind(marker)
        .execute(&env.pool)
        .await
        .unwrap();

    let items = env
        .call(
            Method::GET,
            "/api/admin/markers/pending-edits",
            Some(&admin),
            None,
        )
        .await
        .json();
    let item = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == proposal)
        .unwrap();
    assert_eq!(item["baseMarkerVersion"], 0);
    let current: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id = $1")
        .bind(marker)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(current, 5);
    assert_ne!(item["baseMarkerVersion"], current);
}

#[tokio::test]
async fn admin_user_search_matches_numeric_id_and_escapes_wildcards() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("user-search-admin", "ADMIN").await;
    let (numeric_id, _numeric_public, _) = env.user("numeric-target", "USER").await;
    let (_percent_id, _percent_public, _) = env.user("100%user", "USER").await;
    let (_plain_id, _plain_public, _) = env.user("plain-user", "USER").await;

    let by_id = env
        .call(
            Method::GET,
            &format!("/api/admin/users?q={numeric_id}"),
            Some(&admin),
            None,
        )
        .await
        .json();
    assert_eq!(by_id["totalElements"], 1);
    assert_eq!(by_id["items"][0]["id"], numeric_id);

    let percent = env
        .call(Method::GET, "/api/admin/users?q=%25", Some(&admin), None)
        .await
        .json();
    assert_eq!(percent["totalElements"], 1, "百分号必须按字面量匹配");
    assert_eq!(percent["items"][0]["username"], "100%user");
}

#[tokio::test]
async fn my_submissions_lists_only_own_three_kinds_with_review_notes() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("subs-admin", "ADMIN").await;
    let (_owner_id, owner, owner_cookie) = env.user("subs-owner", "USER").await;
    let (_other_id, other, other_cookie) = env.user("subs-other", "USER").await;

    let marker = seed_marker(
        &env.pool,
        "我的点位",
        "REJECTED",
        &owner,
        "subs-owner",
        "accessible_toilet",
        false,
    )
    .await;
    sqlx::query("UPDATE map_markers SET review_note = '资料不全' WHERE id = $1")
        .bind(marker)
        .execute(&env.pool)
        .await
        .unwrap();
    let edit_marker = seed_marker(
        &env.pool,
        "被编辑",
        "APPROVED",
        &other,
        "subs-other",
        "accessible_toilet",
        false,
    )
    .await;
    let edit = seed_edit(
        &env.pool,
        edit_marker,
        "被编辑",
        "我的编辑",
        &owner,
        "subs-owner",
        0,
        "PENDING",
    )
    .await;
    let image = seed_image(
        &env.pool,
        edit_marker,
        "被编辑",
        &owner,
        "subs-owner",
        "/uploads/markers/mine.png",
        "REJECTED",
    )
    .await;
    sqlx::query("UPDATE marker_image_proposals SET review_note = '图片不清' WHERE id = $1")
        .bind(image)
        .execute(&env.pool)
        .await
        .unwrap();
    let hidden = seed_marker(
        &env.pool,
        "已下架",
        "APPROVED",
        &owner,
        "subs-owner",
        "accessible_toilet",
        true,
    )
    .await;

    let mine = env
        .call(
            Method::GET,
            "/api/markers/me/submissions",
            Some(&owner_cookie),
            None,
        )
        .await;
    assert_eq!(mine.status, StatusCode::OK, "{}", mine.text());
    let mine = mine.json();
    let items = mine.as_array().unwrap();
    assert_eq!(items.len(), 3);
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"marker"));
    assert!(kinds.contains(&"edit"));
    assert!(kinds.contains(&"image"));
    assert!(items.iter().all(|item| item["id"] != hidden));
    let marker_item = items.iter().find(|item| item["kind"] == "marker").unwrap();
    assert_eq!(marker_item["reviewNote"], "资料不全");
    let image_item = items.iter().find(|item| item["kind"] == "image").unwrap();
    assert_eq!(image_item["reviewNote"], "图片不清");
    assert_eq!(image_item["imageUrl"], "/uploads/markers/mine.png");
    let edit_item = items.iter().find(|item| item["kind"] == "edit").unwrap();
    assert_eq!(edit_item["id"], edit);
    assert!(edit_item.get("imageUrl").is_none());

    let theirs = env
        .call(
            Method::GET,
            "/api/markers/me/submissions",
            Some(&other_cookie),
            None,
        )
        .await
        .json();
    assert!(
        theirs
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != marker)
    );
    assert!(
        theirs
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != edit)
    );
    let _ = admin;
}

#[tokio::test]
async fn pending_lists_include_proposer_submission_stats_and_public_dtos_hide_review_note() {
    let env = Env::new().await;
    let (_admin_id, _, admin) = env.user("stats-admin", "ADMIN").await;
    let (_owner_id, owner, owner_cookie) = env.user("stats-owner", "USER").await;

    let pending_marker = seed_marker(
        &env.pool,
        "待审",
        "PENDING",
        &owner,
        "stats-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let approved_marker = seed_marker(
        &env.pool,
        "已通过",
        "APPROVED",
        &owner,
        "stats-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let rejected_marker = seed_marker(
        &env.pool,
        "已拒绝",
        "REJECTED",
        &owner,
        "stats-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit_marker = seed_marker(
        &env.pool,
        "编辑目标",
        "APPROVED",
        &owner,
        "stats-owner",
        "accessible_toilet",
        false,
    )
    .await;
    let edit = seed_edit(
        &env.pool,
        edit_marker,
        "编辑目标",
        "待审编辑",
        &owner,
        "stats-owner",
        0,
        "PENDING",
    )
    .await;
    let image = seed_image(
        &env.pool,
        edit_marker,
        "编辑目标",
        &owner,
        "stats-owner",
        "/uploads/markers/stats.png",
        "PENDING",
    )
    .await;
    sqlx::query("UPDATE map_markers SET review_note = '公开也不返回' WHERE id = $1")
        .bind(approved_marker)
        .execute(&env.pool)
        .await
        .unwrap();

    let markers = env
        .call(
            Method::GET,
            "/api/admin/markers/pending",
            Some(&admin),
            None,
        )
        .await
        .json();
    let item = markers
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == pending_marker)
        .unwrap();
    assert_eq!(item["proposerSubmissionStats"]["approved"], 2);
    assert_eq!(item["proposerSubmissionStats"]["rejected"], 1);
    assert_eq!(item["proposerSubmissionStats"]["pending"], 3);

    let edits = env
        .call(
            Method::GET,
            "/api/admin/markers/pending-edits",
            Some(&admin),
            None,
        )
        .await
        .json();
    let edit_item = edits
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == edit)
        .unwrap();
    assert_eq!(
        edit_item["proposerSubmissionStats"],
        item["proposerSubmissionStats"]
    );

    let images = env
        .call(
            Method::GET,
            "/api/admin/markers/pending-images",
            Some(&admin),
            None,
        )
        .await
        .json();
    let image_item = images
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == image)
        .unwrap();
    assert_eq!(
        image_item["proposerSubmissionStats"],
        item["proposerSubmissionStats"]
    );

    let detail = env
        .call(
            Method::GET,
            &format!("/api/markers/{approved_marker}"),
            None,
            None,
        )
        .await
        .json();
    assert!(detail.get("reviewNote").is_none());
    let public = env
        .call(Method::GET, "/api/markers/public", None, None)
        .await
        .json();
    let public_item = public
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == approved_marker)
        .unwrap();
    assert!(public_item.get("reviewNote").is_none());
    let created = env
        .call(
            Method::GET,
            "/api/markers/me/created",
            Some(&owner_cookie),
            None,
        )
        .await
        .json();
    let created_item = created
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == approved_marker)
        .unwrap();
    assert!(created_item.get("reviewNote").is_none());
    let _ = (rejected_marker, image);
}
