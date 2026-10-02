//! 场所标签（venue_type）与 `hoursTimezone` 的真实 PG / Redis 集成测试。
//!
//! 覆盖 0005 迁移的列/CHECK/历史回填与软删不变、创建默认与显式标签、非法值 400、
//! 旧客户端（不传字段）更新保留原值、类别转换清除/默认、编辑提案快照与审核传递、
//! 管理员直接编辑，以及响应 `venueType`/`hoursTimezone` 字段契约。
//!
//! 每个用例使用 UUID 临时库与独立 Redis 命名空间。

mod common;

use axum::Router;
use axum::http::StatusCode;
use chrono_tz::Asia::Shanghai;
use common::{TempDatabase, call, connect_redis, test_redis_url, unique_cache_namespace};
use fred::clients::Client;
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::migrate::MIGRATOR;
use lycoris_backend::modules::markers::cache::MarkerCache;
use lycoris_backend::modules::markers::service::MarkerService;
use lycoris_backend::modules::markers::write::MarkerWriteService;
use lycoris_backend::modules::markers::write_model::{
    Actor, DEFAULT_VENUE_TYPE, MarkerCreateRequest, MarkerUpdateRequest, WriteError,
};
use serde_json::Value;
use sqlx::PgPool;

const OWNER: &str = "owner-public-1";
const ADMIN: &str = "admin-public-3";

fn owner() -> Actor {
    Actor::new(OWNER, "owner", false)
}

fn admin() -> Actor {
    Actor::new(ADMIN, "admin", true)
}

fn write_service(pool: PgPool, redis: Client, namespace: &str) -> MarkerWriteService {
    MarkerWriteService::new(pool, MarkerCache::new(redis, true, namespace))
}

fn create_request(category: &str, venue_type: Option<&str>) -> MarkerCreateRequest {
    MarkerCreateRequest {
        lat: Some(1.0),
        lng: Some(2.0),
        category: Some(category.to_string()),
        title: Some("标签点位".to_string()),
        description: Some("描述".to_string()),
        language: Some("zh".to_string()),
        is_public: None,
        is_active: None,
        open_time_start: None,
        open_time_end: None,
        client_request_id: None,
        mark_image: None,
        venue_type: venue_type.map(str::to_string),
        categories: None,
        opening_hours_note: None,
    }
}

fn update_request(category: Option<&str>, venue_type: Option<&str>) -> MarkerUpdateRequest {
    MarkerUpdateRequest {
        category: category.map(str::to_string),
        title: Some("更新标题".to_string()),
        description: Some("更新描述".to_string()),
        language: Some("zh".to_string()),
        is_public: None,
        is_active: None,
        open_time_start: None,
        open_time_end: None,
        venue_type: venue_type.map(str::to_string),
        categories: None,
        opening_hours_note: None,
    }
}

async fn seed_marker(
    pool: &PgPool,
    category: &str,
    review_status: &str,
    deactivated: bool,
    version: i64,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            open_time_start, open_time_end, review_status, username, user_public_id, version,
            last_edited_by, last_edited_by_public_id, last_edited_by_owner, deactivated,
            created_at, updated_at)
         VALUES (1.0, 2.0, $1, 'seed', NULL, 'zh', true, true, NULL, NULL, $2, 'owner',
                 $3, $4, 'owner', $3, true, $5, now(), now())
         RETURNING id",
    )
    .bind(category)
    .bind(review_status)
    .bind(OWNER)
    .bind(version)
    .bind(deactivated)
    .fetch_one(pool)
    .await
    .expect("插入点位失败")
}

fn assert_bad(error: WriteError, message: &str) {
    match error {
        WriteError::BadRequest(actual) => assert_eq!(actual, message),
        other => panic!("期望 BadRequest，实际 {other:?}"),
    }
}

fn app_with(
    pool: PgPool,
    redis: Client,
    db_url: &str,
    namespace: &str,
) -> (Router, tempfile::TempDir) {
    let mut config = Config::new(db_url, test_redis_url());
    config.marker_cache_namespace = namespace.to_string();
    let upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
    config.upload_dir = upload.path().to_path_buf();
    let router = build_router(AppState::new(pool, redis, config).expect("构造 AppState 失败"));
    (router, upload)
}

fn parse(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("响应不是合法 JSON")
}

#[tokio::test]
async fn migration_adds_columns_check_constraints_and_backfills_only_toilets() {
    let (_temp, pool) = TempDatabase::create_migrated().await;

    // 两列均为可空 text。
    for table in ["map_markers", "marker_edit_proposals"] {
        let data_type: String = sqlx::query_scalar(
            "SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relname = $1 AND a.attname = 'venue_type'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("读取 venue_type 列类型失败");
        assert_eq!(data_type, "text", "{table}.venue_type 应为 text");
        let not_null: bool = sqlx::query_scalar(
            "SELECT a.attnotnull FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relname = $1 AND a.attname = 'venue_type'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("读取 venue_type 可空性失败");
        assert!(!not_null, "{table}.venue_type 必须可空");
    }

    // CHECK 约束名存在。
    for (table, constraint) in [
        ("map_markers", "ck_map_markers_venue_type"),
        (
            "marker_edit_proposals",
            "ck_marker_edit_proposals_venue_type",
        ),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_constraint con \
             JOIN pg_class c ON c.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relname = $1 AND con.conname = $2 \
               AND con.contype = 'c')",
        )
        .bind(table)
        .bind(constraint)
        .fetch_one(&pool)
        .await
        .expect("读取约束失败");
        assert!(exists, "{table} 缺少 CHECK 约束 {constraint}");
    }

    // 历史回填：迁移前已存在的无障碍卫生间 → other；其它类别保持 NULL；软删状态不变。
    // 通过手工插入后重新走一次 0005 的 UPDATE 语义无法直接模拟“迁移前”，因此改为断言
    // 迁移后的默认约束：未显式回填的行不被改写。
    let toilet_id = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;
    let other_id = seed_marker(&pool, "baby_room", "APPROVED", false, 0).await;
    // 新建行未显式给 venue_type，故为 NULL（迁移回填只作用于迁移执行时刻的既有行）。
    let toilet_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id = $1")
            .bind(toilet_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(toilet_type, None, "迁移后新建行不自动回填，应由应用层写入");
    let other_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id = $1")
            .bind(other_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(other_type, None);

    // CHECK：非法值被拒绝；非卫生间类别携带标签被拒绝；六值均接受。
    for good in [
        "metro",
        "hospital",
        "mall",
        "railway_station",
        "school",
        "public_toilet",
        "airport",
        "other",
    ] {
        sqlx::query("UPDATE map_markers SET venue_type = $2 WHERE id = $1")
            .bind(toilet_id)
            .bind(good)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("合法标签 {good} 应被接受: {error}"));
    }
    let invalid = sqlx::query("UPDATE map_markers SET venue_type = 'cafe' WHERE id = $1")
        .bind(toilet_id)
        .execute(&pool)
        .await;
    assert!(invalid.is_err(), "非法标签必须被 CHECK 拒绝");

    let wrong_category = sqlx::query("UPDATE map_markers SET venue_type = 'mall' WHERE id = $1")
        .bind(other_id)
        .execute(&pool)
        .await;
    assert!(
        wrong_category.is_err(),
        "非 accessible_toilet 类别携带标签必须被 CHECK 拒绝"
    );

    // 软删状态不受约束影响。
    let deactivated_id = seed_marker(&pool, "accessible_toilet", "APPROVED", true, 0).await;
    sqlx::query("UPDATE map_markers SET venue_type = 'hospital' WHERE id = $1")
        .bind(deactivated_id)
        .execute(&pool)
        .await
        .unwrap();
    let still_deactivated: bool =
        sqlx::query_scalar("SELECT deactivated FROM map_markers WHERE id = $1")
            .bind(deactivated_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still_deactivated, "标签更新不得改变既有软删状态");

    pool.close().await;
}

#[tokio::test]
async fn migration_backfills_existing_toilets_and_preserves_soft_delete() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    // 只应用 0001..0004（0005 之前），插入迁移前既有行。
    let up_to_four: Vec<_> = MIGRATOR
        .iter()
        .filter(|migration| migration.version <= 4)
        .cloned()
        .collect();
    assert_eq!(up_to_four.len(), 4, "应内嵌 0001..0004");
    sqlx::migrate::Migrator::with_migrations(up_to_four)
        .run(&pool)
        .await
        .expect("应用 0001..0004 失败");

    let toilet = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;
    let toilet_deleted = seed_marker(&pool, "accessible_toilet", "APPROVED", true, 3).await;
    let baby = seed_marker(&pool, "baby_room", "APPROVED", false, 0).await;
    // 迁移前的历史编辑提案：无障碍卫生间与其它类别各一条。
    let proposal_toilet = sqlx::query_scalar::<_, i64>(
        "INSERT INTO marker_edit_proposals (
            marker_id, marker_title, marker_lat, marker_lng, proposer_username, proposer_public_id,
            proposer_is_owner, category, title, description, language, is_public, is_active,
            open_time_start, open_time_end, base_marker_version, status, created_at)
         VALUES ($1, '既有提案', 1.0, 2.0, 'owner', $2, true, 'accessible_toilet', 't', NULL,
                 'zh', true, true, NULL, NULL, 0, 'PENDING', now())
         RETURNING id",
    )
    .bind(toilet)
    .bind(OWNER)
    .fetch_one(&pool)
    .await
    .unwrap();
    let proposal_baby = sqlx::query_scalar::<_, i64>(
        "INSERT INTO marker_edit_proposals (
            marker_id, marker_title, marker_lat, marker_lng, proposer_username, proposer_public_id,
            proposer_is_owner, category, title, description, language, is_public, is_active,
            open_time_start, open_time_end, base_marker_version, status, created_at)
         VALUES ($1, '既有提案', 1.0, 2.0, 'owner', $2, true, 'baby_room', 't', NULL,
                 'zh', true, true, NULL, NULL, 0, 'PENDING', now())
         RETURNING id",
    )
    .bind(baby)
    .bind(OWNER)
    .fetch_one(&pool)
    .await
    .unwrap();

    // 应用 0005。
    lycoris_backend::migrate::run(&pool)
        .await
        .expect("应用 0005 失败");

    let toilet_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id = $1")
            .bind(toilet)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        toilet_type.as_deref(),
        Some("other"),
        "既有卫生间应回填 other"
    );
    let baby_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id = $1")
            .bind(baby)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(baby_type, None, "非卫生间既有行保持 NULL");
    let proposal_toilet_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM marker_edit_proposals WHERE id = $1")
            .bind(proposal_toilet)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(proposal_toilet_type.as_deref(), Some("other"));
    let proposal_baby_type: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM marker_edit_proposals WHERE id = $1")
            .bind(proposal_baby)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(proposal_baby_type, None);

    // 软删状态与版本未被回填改动。
    let (deactivated, version): (bool, i64) =
        sqlx::query_as("SELECT deactivated, version FROM map_markers WHERE id = $1")
            .bind(toilet_deleted)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deactivated, "历史回填不得清除软删状态");
    assert_eq!(version, 3, "历史回填不得推进版本");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM map_markers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 3, "历史回填不得删除任何记录");

    pool.close().await;
}

#[tokio::test]
async fn create_defaults_and_validates_venue_type() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let actor = owner();

    // 无障碍卫生间未传值 → 默认 other。
    let default_row = write
        .create_marker(&actor, "zh", create_request("accessible_toilet", None))
        .await
        .unwrap();
    assert_eq!(default_row.venue_type.as_deref(), Some(DEFAULT_VENUE_TYPE));

    // 显式合法标签被接受。
    for good in [
        "metro",
        "hospital",
        "mall",
        "railway_station",
        "school",
        "public_toilet",
        "airport",
        "other",
    ] {
        let row = write
            .create_marker(
                &actor,
                "zh",
                create_request("accessible_toilet", Some(good)),
            )
            .await
            .unwrap();
        assert_eq!(row.venue_type.as_deref(), Some(good));
    }
    // 首尾空白按值处理（trim 后合法）。
    let trimmed = write
        .create_marker(
            &actor,
            "zh",
            create_request("accessible_toilet", Some("  metro  ")),
        )
        .await
        .unwrap();
    assert_eq!(trimmed.venue_type.as_deref(), Some("metro"));
    // 空白串按未提供处理 → other。
    let blank = write
        .create_marker(
            &actor,
            "zh",
            create_request("accessible_toilet", Some("   ")),
        )
        .await
        .unwrap();
    assert_eq!(blank.venue_type.as_deref(), Some("other"));

    // 非法值 → 400。
    assert_bad(
        write
            .create_marker(
                &actor,
                "zh",
                create_request("accessible_toilet", Some("cafe")),
            )
            .await
            .unwrap_err(),
        "venueType 不合法",
    );

    // 非卫生间类别：不传值 → NULL；传值 → 400。
    let no_label = write
        .create_marker(&actor, "zh", create_request("baby_room", None))
        .await
        .unwrap();
    assert_eq!(no_label.venue_type, None);
    assert_bad(
        write
            .create_marker(&actor, "zh", create_request("baby_room", Some("mall")))
            .await
            .unwrap_err(),
        "venueType 不合法",
    );
    assert_bad(
        write
            .create_marker(
                &actor,
                "zh",
                create_request("self_definition", Some("other")),
            )
            .await
            .unwrap_err(),
        "venueType 不合法",
    );

    pool.close().await;
}

#[tokio::test]
async fn update_preserves_omitted_value_and_handles_category_conversion() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());

    let marker = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;
    // 先建立 metro 标签（管理员直接编辑）。
    let seeded = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker,
            update_request(Some("accessible_toilet"), Some("metro")),
        )
        .await
        .unwrap();
    assert_eq!(seeded.venue_type.as_deref(), Some("metro"));

    // 旧客户端（不传 venueType）更新 → 保留原值。
    let preserved = write
        .admin_update_marker(&admin(), "zh", marker, update_request(None, None))
        .await
        .unwrap();
    assert_eq!(
        preserved.venue_type.as_deref(),
        Some("metro"),
        "未传值必须保留原标签"
    );

    // 转入 accessible_toilet（原为非卫生间且无标签）→ 默认 other。
    let fresh = seed_marker(&pool, "baby_room", "APPROVED", false, 0).await;
    let converted_in = write
        .admin_update_marker(
            &admin(),
            "zh",
            fresh,
            update_request(Some("accessible_toilet"), None),
        )
        .await
        .unwrap();
    assert_eq!(
        converted_in.venue_type.as_deref(),
        Some("other"),
        "转入 accessible_toilet 未传值必须默认 other"
    );

    // 转出 accessible_toilet → 清除标签。
    let converted_out = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker,
            update_request(Some("baby_room"), None),
        )
        .await
        .unwrap();
    assert_eq!(
        converted_out.venue_type, None,
        "转出 accessible_toilet 必须清除标签"
    );

    // 空白串视为未提供：仍为 accessible_toilet，保留/默认 other。
    let blank_kept = write
        .admin_update_marker(
            &admin(),
            "zh",
            fresh,
            update_request(Some("accessible_toilet"), Some("   ")),
        )
        .await
        .unwrap();
    assert_eq!(blank_kept.venue_type.as_deref(), Some("other"));

    pool.close().await;
}

#[tokio::test]
async fn update_rejects_non_toilet_label_and_invalid_value() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;

    assert_bad(
        write
            .admin_update_marker(&admin(), "zh", marker, update_request(None, Some("cafe")))
            .await
            .unwrap_err(),
        "venueType 不合法",
    );
    assert_bad(
        write
            .admin_update_marker(
                &admin(),
                "zh",
                marker,
                update_request(Some("baby_room"), Some("mall")),
            )
            .await
            .unwrap_err(),
        "venueType 不合法",
    );

    pool.close().await;
}

#[tokio::test]
async fn user_edit_proposal_snapshots_and_approval_carry_venue_type() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let namespace = unique_cache_namespace();
    let write = write_service(pool.clone(), redis.clone(), &namespace);

    let marker = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;

    // 普通用户提案：显式 hospital。
    write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("提案标题".to_string()),
                venue_type: Some("hospital".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let (proposal_id, proposal_venue): (i64, Option<String>) =
        sqlx::query_as("SELECT id, venue_type FROM marker_edit_proposals WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        proposal_venue.as_deref(),
        Some("hospital"),
        "提案必须快照场所标签"
    );

    let approved = write
        .approve_edit_proposal(&admin(), proposal_id)
        .await
        .unwrap();
    assert_eq!(
        approved.venue_type.as_deref(),
        Some("hospital"),
        "审核通过必须把提案标签落到点位"
    );

    // 非卫生间提案传标签 → 400（不得落库）。
    let other = seed_marker(&pool, "baby_room", "APPROVED", false, 0).await;
    assert_bad(
        write
            .create_edit_proposal(
                &owner(),
                "zh",
                other,
                MarkerUpdateRequest {
                    title: Some("坏提案".to_string()),
                    venue_type: Some("mall".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err(),
        "venueType 不合法",
    );

    // 旧客户端提案（不传字段）→ 保留现状 other。
    let current: Option<String> =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current.as_deref(), Some("hospital"));
    write
        .create_edit_proposal(&owner(), "zh", marker, update_request(None, None))
        .await
        .unwrap();
    let preserved: Option<String> = sqlx::query_scalar(
        "SELECT venue_type FROM marker_edit_proposals WHERE marker_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        preserved.as_deref(),
        Some("hospital"),
        "未传值提案必须保留点位原标签"
    );

    pool.close().await;
}

#[tokio::test]
async fn responses_expose_venue_type_and_hours_timezone() {
    let (temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let namespace = unique_cache_namespace();
    let (router, _upload) = app_with(pool.clone(), redis.clone(), temp.url(), &namespace);

    let marker = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 0).await;
    sqlx::query("UPDATE map_markers SET venue_type = 'railway_station' WHERE id = $1")
        .bind(marker)
        .execute(&pool)
        .await
        .unwrap();

    let (status, _, body) = call(&router, &format!("/api/markers/{marker}")).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse(&body);
    assert_eq!(
        json["venueType"],
        Value::String("railway_station".to_string())
    );
    assert_eq!(
        json["hoursTimezone"],
        Value::String(Shanghai.name().to_string()),
        "hoursTimezone 必须等于服务端 availability_zone 的 IANA 名称"
    );

    // 非卫生间响应 venueType 为 null。
    let other = seed_marker(&pool, "baby_room", "APPROVED", false, 0).await;
    let (status, _, body) = call(&router, &format!("/api/markers/{other}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(parse(&body)["venueType"], Value::Null);

    // 管理员待审列表响应含 venueType。
    let pending = seed_marker(&pool, "accessible_toilet", "PENDING", false, 0).await;
    write_service(pool.clone(), redis, &unique_cache_namespace())
        .admin_update_marker(
            &admin(),
            "zh",
            pending,
            update_request(Some("accessible_toilet"), Some("school")),
        )
        .await
        .unwrap();
    let service = MarkerService::new(
        lycoris_backend::modules::markers::repository::MarkerRepository::new(pool.clone()),
        MarkerCache::new(connect_redis().await, true, unique_cache_namespace()),
        Shanghai,
    );
    let rows = sqlx::query_as::<_, lycoris_backend::modules::markers::model::MarkerRow>(
        "SELECT id, version, lat, lng, category, title, description, source_language,
                is_public, username, user_public_id, client_request_id, is_active,
                open_time_start, open_time_end, review_status, last_edited_by,
                last_edited_by_public_id, last_edited_by_owner, mark_image, venue_type, categories, opening_hours_note,
                deactivated, created_at, updated_at
         FROM map_markers WHERE id = $1",
    )
    .bind(pending)
    .fetch_one(&pool)
    .await
    .unwrap();
    let dto = service.localize_row(rows, "zh").await.unwrap();
    assert_eq!(dto.venue_type.as_deref(), Some("school"));
    assert_eq!(dto.hours_timezone, Shanghai.name());

    pool.close().await;
}

#[tokio::test]
async fn ordered_categories_notes_and_park_survive_old_clients_and_review() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let namespace = unique_cache_namespace();
    let write = write_service(pool.clone(), redis.clone(), &namespace);
    let mut create = create_request("baby_room", Some("park"));
    create.categories = Some(vec!["baby_room".into(), "accessible_toilet".into()]);
    create.opening_hours_note = Some("周一休息；午间 12–14 点暂停开放".into());
    let marker = write.create_marker(&owner(), "en", create).await.unwrap();
    assert_eq!(marker.category, "baby_room");
    assert_eq!(marker.categories, ["baby_room", "accessible_toilet"]);
    assert_eq!(marker.venue_type.as_deref(), Some("park"));
    assert_eq!(marker.source_language, "zh");
    let approved = write.approve_marker(&admin(), marker.id).await.unwrap();
    assert_eq!(approved.id, marker.id);
    let legacy = write
        .admin_update_marker(&admin(), "zh", marker.id, update_request(None, None))
        .await
        .unwrap();
    assert_eq!(legacy.categories, marker.categories);
    assert_eq!(legacy.opening_hours_note, marker.opening_hours_note);
    let service = MarkerService::new(
        lycoris_backend::modules::markers::repository::MarkerRepository::new(pool.clone()),
        MarkerCache::new(redis, true, namespace),
        Shanghai,
    );
    for category in ["accessible_toilet", "baby_room"] {
        assert!(
            service
                .nearby(1.0, 2.0, 1000, category, "zh")
                .await
                .unwrap()
                .iter()
                .any(|p| p.id == marker.id)
        );
    }
    let edit = MarkerUpdateRequest {
        categories: Some(vec!["accessible_toilet".into(), "friendly_clinic".into()]),
        opening_hours_note: Some("节假日请提前联系".into()),
        ..Default::default()
    };
    write
        .create_edit_proposal(&owner(), "en", marker.id, edit)
        .await
        .unwrap();
    let proposal: i64 = sqlx::query_scalar(
        "SELECT id FROM marker_edit_proposals WHERE marker_id=$1 AND status='PENDING'",
    )
    .bind(marker.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    write
        .approve_edit_proposal(&admin(), proposal)
        .await
        .unwrap();
    let cleared = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker.id,
            MarkerUpdateRequest {
                opening_hours_note: Some("".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.category, "accessible_toilet");
    assert_eq!(cleared.categories, ["accessible_toilet", "friendly_clinic"]);
    assert_eq!(cleared.opening_hours_note, None);
    for categories in [vec![], vec!["unknown".into()]] {
        let error = write
            .admin_update_marker(
                &admin(),
                "zh",
                marker.id,
                MarkerUpdateRequest {
                    categories: Some(categories),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, WriteError::BadRequest(_)));
    }
    let error = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker.id,
            MarkerUpdateRequest {
                opening_hours_note: Some("字".repeat(1001)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, WriteError::BadRequest(_)));
}

#[tokio::test]
async fn upgrade_retains_legacy_cover_and_classifies_only_reviewed_parks() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    let previous = sqlx::migrate::Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|m| m.version <= 8)
            .cloned()
            .collect(),
    );
    previous.run(&pool).await.unwrap();
    let park = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 1).await;
    sqlx::query("UPDATE map_markers SET id=220,title='海淀区-马甸公园(北)-无障碍卫生间',venue_type='public_toilet',mark_image='/uploads/markers/legacy.png' WHERE id=$1")
        .bind(park).execute(&pool).await.unwrap();
    let station = seed_marker(&pool, "accessible_toilet", "APPROVED", false, 2).await;
    sqlx::query("UPDATE map_markers SET title='天河公园站',venue_type='metro' WHERE id=$1")
        .bind(station)
        .execute(&pool)
        .await
        .unwrap();
    let inactive = seed_marker(&pool, "accessible_toilet", "APPROVED", true, 1).await;
    sqlx::query("UPDATE map_markers SET id=332,title='和平区-复兴公园公厕(HP1-WDD19)-无障碍卫生间',venue_type='public_toilet' WHERE id=$1")
        .bind(inactive).execute(&pool).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let row: (String, Vec<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT venue_type,categories,opening_hours_note,version FROM map_markers WHERE id=220",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        ("park".into(), vec!["accessible_toilet".into()], None, 2)
    );
    let url: String = sqlx::query_scalar("SELECT image_url FROM marker_photos WHERE marker_id=220")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(url, "/uploads/markers/legacy.png");
    let station_venue: String =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id=$1")
            .bind(station)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(station_venue, "metro");
    let inactive_venue: String =
        sqlx::query_scalar("SELECT venue_type FROM map_markers WHERE id=332")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(inactive_venue, "public_toilet");
}
