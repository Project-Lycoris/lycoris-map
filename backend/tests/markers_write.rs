//! 阶段 3 点位写入/审核事务核心的真实 PG / Redis 集成测试。
//!
//! 测试直接调用 `MarkerWriteService`（不伪造 HTTP 认证路由），在 UUID 临时库与独立 Redis
//! 命名空间上验证业务效果与并发约束：幂等创建与收藏、收藏/删除竞争、可见性、提案审核一次性、
//! 同基准提案竞争、版本仲裁、强制译文失败回滚、软删除与关联数据留存、缓存失效与故障提交。

mod common;

use std::time::{Duration, Instant};

use common::{TempDatabase, connect_redis, unique_cache_namespace, unreachable_redis};
use fred::clients::Client;
use fred::interfaces::KeysInterface;
use lycoris_backend::modules::markers::cache::MarkerCache;
use lycoris_backend::modules::markers::localization::{
    localize_text, source_hash_components, translation_is_current,
};
use lycoris_backend::modules::markers::model::TranslationRow;
use lycoris_backend::modules::markers::write::MarkerWriteService;
use lycoris_backend::modules::markers::write_model::{
    Actor, MarkerCreateRequest, MarkerUpdateRequest, WriteError,
};
use sqlx::PgPool;

const OWNER: &str = "owner-public-1";
const OTHER: &str = "other-public-2";
const ADMIN: &str = "admin-public-3";

fn owner() -> Actor {
    Actor::new(OWNER, "owner", false)
}

fn other() -> Actor {
    Actor::new(OTHER, "other", false)
}

fn admin() -> Actor {
    Actor::new(ADMIN, "admin", true)
}

fn write_service(pool: PgPool, redis: Client, namespace: &str) -> MarkerWriteService {
    MarkerWriteService::new(pool, MarkerCache::new(redis, true, namespace))
}

#[derive(Clone)]
struct Seed {
    lat: f64,
    lng: f64,
    category: String,
    title: String,
    description: Option<String>,
    source_language: String,
    is_public: bool,
    review_status: String,
    version: i64,
    user_public_id: Option<String>,
    open_time_start: Option<String>,
    open_time_end: Option<String>,
}

impl Default for Seed {
    fn default() -> Self {
        Self {
            lat: 1.0,
            lng: 2.0,
            category: "accessible_toilet".to_string(),
            title: "seed".to_string(),
            description: None,
            source_language: "zh".to_string(),
            is_public: true,
            review_status: "APPROVED".to_string(),
            version: 0,
            user_public_id: Some(OWNER.to_string()),
            open_time_start: None,
            open_time_end: None,
        }
    }
}

async fn seed_marker(pool: &PgPool, seed: Seed) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            open_time_start, open_time_end, review_status, username, user_public_id, version,
            last_edited_by, last_edited_by_public_id, last_edited_by_owner, created_at, updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,true,$8,$9,$10,$11,$12,$13,$11,$12,true,now(),now())
         RETURNING id",
    )
    .bind(seed.lat)
    .bind(seed.lng)
    .bind(seed.category)
    .bind(seed.title)
    .bind(seed.description)
    .bind(seed.source_language)
    .bind(seed.is_public)
    .bind(seed.open_time_start)
    .bind(seed.open_time_end)
    .bind(seed.review_status)
    .bind("seed-user")
    .bind(seed.user_public_id)
    .bind(seed.version)
    .fetch_one(pool)
    .await
    .expect("插入点位失败")
}

async fn seed_translation(
    pool: &PgPool,
    marker_id: i64,
    language: &str,
    title: &str,
    description: Option<&str>,
    source_hash: &str,
) {
    sqlx::query(
        "INSERT INTO map_marker_translations
            (marker_id, language, title, description, source_hash, origin, updated_at)
         VALUES ($1,$2,$3,$4,$5,'MACHINE',now())",
    )
    .bind(marker_id)
    .bind(language)
    .bind(title)
    .bind(description)
    .bind(source_hash)
    .execute(pool)
    .await
    .expect("插入译文失败");
}

async fn count(pool: &PgPool, sql: &'static str, binds: &[i64]) -> i64 {
    let mut query = sqlx::query_scalar::<_, i64>(sql);
    for value in binds {
        query = query.bind(*value);
    }
    query.fetch_one(pool).await.expect("计数查询失败")
}

fn valid_create() -> MarkerCreateRequest {
    MarkerCreateRequest {
        lat: Some(1.0),
        lng: Some(2.0),
        category: Some("accessible_toilet".to_string()),
        title: Some("新点位".to_string()),
        description: Some("描述".to_string()),
        language: Some("zh".to_string()),
        is_public: None,
        is_active: None,
        open_time_start: None,
        open_time_end: None,
        client_request_id: None,
        mark_image: None,
        venue_type: None,
        categories: None,
        opening_hours_note: None,
    }
}

fn assert_bad(error: WriteError, message: &str) {
    match error {
        WriteError::BadRequest(actual) => assert_eq!(actual, message),
        other => panic!("期望 BadRequest，实际 {other:?}"),
    }
}

#[tokio::test]
async fn create_validates_required_fields_ranges_lengths_and_open_time() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let actor = owner();

    for (req, message) in [
        (
            {
                let mut r = valid_create();
                r.lat = None;
                r
            },
            "缺少必要字段",
        ),
        (
            {
                let mut r = valid_create();
                r.lng = None;
                r
            },
            "缺少必要字段",
        ),
        (
            {
                let mut r = valid_create();
                r.category = None;
                r
            },
            "缺少必要字段",
        ),
        (
            {
                let mut r = valid_create();
                r.title = None;
                r
            },
            "缺少必要字段",
        ),
    ] {
        assert_bad(
            write.create_marker(&actor, "zh", req).await.unwrap_err(),
            message,
        );
    }

    for (lat, lng) in [
        (91.0, 2.0),
        (1.0, 181.0),
        (f64::NAN, 2.0),
        (1.0, f64::INFINITY),
    ] {
        let mut req = valid_create();
        req.lat = Some(lat);
        req.lng = Some(lng);
        assert_bad(
            write.create_marker(&actor, "zh", req).await.unwrap_err(),
            "lat/lng 不合法",
        );
    }

    let mut bad_category = valid_create();
    bad_category.category = Some("unknown".to_string());
    assert!(
        matches!(
            write.create_marker(&actor, "zh", bad_category).await.unwrap_err(),
            WriteError::BadRequest(message) if message.starts_with("不支持的 category")
        ),
        "未知 category 应 400"
    );

    let mut long_title = valid_create();
    long_title.title = Some("标".repeat(121));
    assert_bad(
        write
            .create_marker(&actor, "zh", long_title)
            .await
            .unwrap_err(),
        "title 过长",
    );

    let mut long_client_id = valid_create();
    long_client_id.client_request_id = Some("a".repeat(65));
    assert_bad(
        write
            .create_marker(&actor, "zh", long_client_id)
            .await
            .unwrap_err(),
        "clientRequestId 过长",
    );

    let mut one_sided = valid_create();
    one_sided.open_time_start = Some("08:00".to_string());
    assert_bad(
        write
            .create_marker(&actor, "zh", one_sided)
            .await
            .unwrap_err(),
        "请同时填写开始和结束时间，或都留空",
    );

    let mut bad_time = valid_create();
    bad_time.open_time_start = Some("8:00".to_string());
    bad_time.open_time_end = Some("20:00".to_string());
    assert_bad(
        write
            .create_marker(&actor, "zh", bad_time)
            .await
            .unwrap_err(),
        "时间格式不合法，请使用 HH:mm",
    );

    // 合法创建：默认公开、isActive=true、PENDING，clientRequestId trim，空串归 null。
    let mut ok = valid_create();
    ok.client_request_id = Some("  key-1  ".to_string());
    ok.open_time_start = Some("08:00".to_string());
    ok.open_time_end = Some("20:00".to_string());
    let row = write.create_marker(&actor, "zh", ok).await.unwrap();
    assert_eq!(row.review_status, "PENDING");
    assert!(row.is_public);
    assert!(row.is_active);
    assert_eq!(row.version, 0);
    assert_eq!(row.source_language, "zh");
    assert_eq!(row.client_request_id.as_deref(), Some("key-1"));
    assert_eq!(row.open_time_start.as_deref(), Some("08:00"));
    assert_eq!(row.user_public_id.as_deref(), Some(OWNER));
    assert_eq!(row.username, "owner");

    let mut blank = valid_create();
    blank.client_request_id = Some("   ".to_string());
    blank.title = Some("标".repeat(120));
    let blank_row = write.create_marker(&actor, "zh", blank).await.unwrap();
    assert_eq!(blank_row.client_request_id, None, "空白幂等键归 null");

    // 无幂等键不会错误去重。
    let first = write
        .create_marker(&actor, "zh", valid_create())
        .await
        .unwrap();
    let second = write
        .create_marker(&actor, "zh", valid_create())
        .await
        .unwrap();
    assert_ne!(first.id, second.id, "没有 clientRequestId 不得去重");

    pool.close().await;
}

#[tokio::test]
async fn concurrent_create_with_same_client_request_id_returns_same_marker() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let actor = owner();

    let mut req = valid_create();
    req.client_request_id = Some("dup-key".to_string());
    let (left, right) = tokio::join!(
        write.create_marker(&actor, "zh", req.clone()),
        write.create_marker(&actor, "zh", req.clone()),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.id, right.id, "并发同 key 应返回同一 ID");

    let stored = count(
        &pool,
        "SELECT count(*) FROM map_markers WHERE client_request_id = 'dup-key'",
        &[],
    )
    .await;
    assert_eq!(stored, 1, "同 key 只应有一行");

    pool.close().await;
}

#[tokio::test]
async fn create_replay_with_existing_key_ignores_changed_payload() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let actor = owner();

    let mut req = valid_create();
    req.client_request_id = Some("replay-key".to_string());
    let original = write
        .create_marker(&actor, "zh", req.clone())
        .await
        .unwrap();

    // 同一 key 携带非法 category、单边开放时间与不同标题：仍直接返回原点位。
    let mut changed = valid_create();
    changed.client_request_id = Some("replay-key".to_string());
    changed.category = Some("unknown".to_string());
    changed.open_time_start = Some("08:00".to_string());
    changed.title = Some("完全不同的标题".to_string());
    let replay = write.create_marker(&actor, "zh", changed).await.unwrap();
    assert_eq!(replay.id, original.id, "已有 key 必须返回同一 ID");
    assert_eq!(replay.title, original.title, "重放不得修改原点位标题");
    assert_eq!(replay.category, original.category);
    assert_eq!(replay.version, original.version);

    // 重放仍先做最小必填字段检查：缺 title → 400，且不改变原点位。
    let mut missing = valid_create();
    missing.client_request_id = Some("replay-key".to_string());
    missing.title = None;
    assert_bad(
        write
            .create_marker(&actor, "zh", missing)
            .await
            .unwrap_err(),
        "缺少必要字段",
    );
    let rows = count(
        &pool,
        "SELECT count(*) FROM map_markers WHERE client_request_id = 'replay-key'",
        &[],
    )
    .await;
    assert_eq!(rows, 1);
    let title: String =
        sqlx::query_scalar("SELECT title FROM map_markers WHERE client_request_id = 'replay-key'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, original.title);

    pool.close().await;
}

#[tokio::test]
async fn favorites_are_idempotent_and_respect_visibility() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());

    let public = seed_marker(&pool, Seed::default()).await;
    let private = seed_marker(
        &pool,
        Seed {
            is_public: false,
            title: "private".to_string(),
            ..Default::default()
        },
    )
    .await;
    let pending = seed_marker(
        &pool,
        Seed {
            review_status: "PENDING".to_string(),
            title: "pending".to_string(),
            ..Default::default()
        },
    )
    .await;

    // 幂等：属主与访客重复收藏都成功，且只有一条。
    write.add_favorite(&owner(), public).await.unwrap();
    write.add_favorite(&owner(), public).await.unwrap();
    write.add_favorite(&other(), public).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM marker_favorites WHERE marker_id = $1",
            &[public]
        )
        .await,
        2
    );

    // 可见性：私有/待审非属主不可见 → 404，管理员与属主可见。
    assert!(matches!(
        write.add_favorite(&other(), private).await.unwrap_err(),
        WriteError::NotFound(_)
    ));
    assert!(matches!(
        write.add_favorite(&other(), pending).await.unwrap_err(),
        WriteError::NotFound(_)
    ));
    write.add_favorite(&admin(), private).await.unwrap();
    write.add_favorite(&owner(), pending).await.unwrap();

    // 取消收藏不要求点位存在。
    write.remove_favorite(&owner(), 999_999).await.unwrap();
    write.remove_favorite(&owner(), public).await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marker_favorites WHERE marker_id = $1 AND user_public_id = $2",
    )
    .bind(public)
    .bind(OTHER)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1, "取消后只剩 other 的收藏");

    pool.close().await;
}

#[tokio::test]
async fn favorite_and_delete_race_leaves_no_orphan() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(&pool, Seed::default()).await;

    let actor = owner();
    let (favorite, delete) = tokio::join!(
        write.add_favorite(&actor, marker),
        write.delete_owned_marker(&actor, marker),
    );
    assert!(delete.is_ok(), "属主删除应成功");
    assert!(
        favorite.is_ok() || matches!(favorite, Err(WriteError::NotFound(_))),
        "收藏要么成功要么点位不存在，不能其它错误"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM marker_favorites f LEFT JOIN map_markers m ON m.id = f.marker_id WHERE m.id IS NULL",
            &[],
        )
        .await,
        0,
        "不得留下孤立收藏"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_markers WHERE id = $1 AND deactivated = false",
            &[marker]
        )
        .await,
        0
    );

    pool.close().await;
}

#[tokio::test]
async fn non_owner_delete_is_forbidden() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(&pool, Seed::default()).await;

    assert!(matches!(
        write
            .delete_owned_marker(&other(), marker)
            .await
            .unwrap_err(),
        WriteError::Forbidden
    ));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_markers WHERE id = $1",
            &[marker]
        )
        .await,
        1,
        "非属主删除必须拒绝且保留点位"
    );

    pool.close().await;
}

#[tokio::test]
async fn owner_delete_preserves_all_data_and_admin_can_restore() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(&pool, Seed::default()).await;
    seed_translation(&pool, marker, "en", "EN", Some("d"), "hash").await;
    write.add_favorite(&owner(), marker).await.unwrap();
    write.add_favorite(&other(), marker).await.unwrap();
    let _ = write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("提案".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    write.delete_owned_marker(&owner(), marker).await.unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_markers WHERE id = $1",
            &[marker]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM marker_favorites WHERE marker_id = $1",
            &[marker]
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_marker_translations WHERE marker_id = $1",
            &[marker]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM marker_edit_proposals WHERE marker_id = $1",
            &[marker]
        )
        .await,
        1,
        "历史提案必须留存"
    );

    let disabled = write.list_all_markers(&admin()).await.unwrap().remove(0);
    assert_eq!(disabled.version, 1);
    write.delete_owned_marker(&owner(), marker).await.unwrap();
    write.admin_delete_marker(&admin(), marker).await.unwrap();
    assert_eq!(
        write.list_all_markers(&admin()).await.unwrap()[0].version,
        1
    );
    let proposal: i64 =
        sqlx::query_scalar("SELECT id FROM marker_edit_proposals WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        write
            .approve_edit_proposal(&admin(), proposal)
            .await
            .is_err()
    );
    assert!(
        write
            .pending_edit_proposals(&admin())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(write.favorite_ids(&owner()).await.unwrap().is_empty());
    assert!(write.list_created(&owner()).await.unwrap().is_empty());
    assert!(
        write
            .list_all_markers(&admin())
            .await
            .unwrap()
            .iter()
            .any(|m| m.id == marker && m.deactivated)
    );
    assert!(matches!(
        write.admin_restore_marker(&owner(), marker).await,
        Err(WriteError::Forbidden)
    ));
    write.admin_restore_marker(&admin(), marker).await.unwrap();
    assert_eq!(write.favorite_ids(&owner()).await.unwrap(), vec![marker]);
    let restored = write.list_created(&owner()).await.unwrap();
    assert_eq!(restored.len(), 1);
    assert!(!restored[0].deactivated);
    assert_eq!(restored[0].review_status, "APPROVED");
    assert!(restored[0].is_public);
    assert_eq!(restored[0].version, 2);
    write.admin_restore_marker(&admin(), marker).await.unwrap();
    assert_eq!(
        write.list_all_markers(&admin()).await.unwrap()[0].version,
        2
    );
    assert!(
        write
            .approve_edit_proposal(&admin(), proposal)
            .await
            .is_err(),
        "恢复不能放行基于旧版本的编辑提案"
    );
    pool.close().await;
}

#[tokio::test]
async fn user_patch_creates_pending_proposal_only() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(
        &pool,
        Seed {
            title: "旧标题".to_string(),
            version: 5,
            ..Default::default()
        },
    )
    .await;

    let returned = write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                category: Some("baby_room".to_string()),
                title: Some("新标题".to_string()),
                description: Some("新描述".to_string()),
                language: Some("zh".to_string()),
                is_public: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(returned.version, 5, "PATCH 不得改动点位版本");
    assert_eq!(returned.title, "旧标题");
    assert_eq!(returned.review_status, "APPROVED");

    let proposal: (String, i64, bool, String, String, Option<String>, String, bool) =
        sqlx::query_as(
            "SELECT status, base_marker_version, proposer_is_owner, category, title, description, language, is_public
             FROM marker_edit_proposals WHERE marker_id = $1",
        )
        .bind(marker)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(proposal.0, "PENDING");
    assert_eq!(proposal.1, 5, "必须记录 base_marker_version");
    assert!(proposal.2, "属主提案 proposer_is_owner 为 true");
    assert_eq!(proposal.3, "baby_room");
    assert_eq!(proposal.4, "新标题");
    assert_eq!(proposal.5.as_deref(), Some("新描述"));
    assert_eq!(proposal.6, "zh");
    assert!(!proposal.7);

    // 非属主也可提案（普通用户），proposer_is_owner 为 false。
    let _ = write
        .create_edit_proposal(
            &other(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("他人提案".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let owner_flag: bool = sqlx::query_scalar(
        "SELECT proposer_is_owner FROM marker_edit_proposals WHERE marker_id = $1 AND title = '他人提案'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!owner_flag);

    pool.close().await;
}

#[tokio::test]
async fn public_edit_uses_chinese_source_regardless_of_request_language() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let write = write_service(
        pool.clone(),
        connect_redis().await,
        &unique_cache_namespace(),
    );
    let marker = seed_marker(
        &pool,
        Seed {
            title: "原文".into(),
            description: Some("原描述".into()),
            ..Default::default()
        },
    )
    .await;
    write
        .create_edit_proposal(
            &owner(),
            "en",
            marker,
            MarkerUpdateRequest {
                title: Some("中文更新".into()),
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposal = write
        .pending_edit_proposals(&admin())
        .await
        .unwrap()
        .remove(0);
    assert_eq!(proposal.language, "zh");
    assert_eq!(proposal.description.as_deref(), Some("原描述"));
    let saved = write
        .approve_edit_proposal(&admin(), proposal.id)
        .await
        .unwrap();
    assert_eq!(saved.source_language, "zh");
    assert_eq!(saved.title, "中文更新");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_marker_translations WHERE marker_id=$1",
            &[marker]
        )
        .await,
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn admin_pending_lists_and_direct_review() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());

    let pending = seed_marker(
        &pool,
        Seed {
            review_status: "PENDING".to_string(),
            title: "pending".to_string(),
            version: 2,
            ..Default::default()
        },
    )
    .await;
    seed_marker(
        &pool,
        Seed {
            title: "approved".to_string(),
            ..Default::default()
        },
    )
    .await;

    assert!(matches!(
        write.pending_markers(&other()).await.unwrap_err(),
        WriteError::Forbidden
    ));
    let list = write.pending_markers(&admin()).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, pending);

    let approved = write.approve_marker(&admin(), pending).await.unwrap();
    assert_eq!(approved.review_status, "APPROVED");
    assert_eq!(approved.version, 3, "直接审核必须推进版本");

    let pending2 = seed_marker(
        &pool,
        Seed {
            review_status: "PENDING".to_string(),
            title: "reject me".to_string(),
            version: 0,
            ..Default::default()
        },
    )
    .await;
    let rejected = write.reject_marker(&admin(), pending2).await.unwrap();
    assert_eq!(rejected.review_status, "REJECTED");
    assert_eq!(rejected.version, 1);

    assert!(matches!(
        write.approve_marker(&other(), pending).await.unwrap_err(),
        WriteError::Forbidden
    ));
    assert!(matches!(
        write
            .approve_marker(&admin(), 999_999)
            .await
            .unwrap_err(),
        WriteError::NotFound(message) if message == "点位不存在"
    ));

    pool.close().await;
}

#[tokio::test]
async fn concurrent_admin_approval_is_one_time() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(
        &pool,
        Seed {
            review_status: "PENDING".to_string(),
            version: 4,
            ..Default::default()
        },
    )
    .await;
    write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("审核版".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposal: i64 =
        sqlx::query_scalar("SELECT id FROM marker_edit_proposals WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();

    let admin_actor = admin();
    let admin2 = Actor::new("admin-public-4", "admin2", true);
    let (first, second) = tokio::join!(
        write.approve_edit_proposal(&admin_actor, proposal),
        write.approve_edit_proposal(&admin2, proposal),
    );
    let successes = [first.is_ok(), second.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1, "同一提案并发审核只允许一人成功");
    let loser = if first.is_err() { first } else { second };
    match loser.unwrap_err() {
        WriteError::BadRequest(message) => assert_eq!(message, "该提案已处理"),
        other => panic!("期望 400 该提案已处理，实际 {other:?}"),
    }
    let version: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id = $1")
        .bind(marker)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 5, "只应推进一次版本");

    pool.close().await;
}

#[tokio::test]
async fn two_proposals_with_same_base_race_one_conflicts() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(
        &pool,
        Seed {
            title: "基准".to_string(),
            version: 3,
            ..Default::default()
        },
    )
    .await;

    // 一个原文提案、一个译文提案，基准版本相同；原文/翻译并发共用同一 marker.version 仲裁。
    write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("源提案".to_string()),
                language: Some("zh".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("EN 提案".to_string()),
                description: Some("EN desc".to_string()),
                language: Some("en".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposals: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM marker_edit_proposals WHERE marker_id = $1 ORDER BY id ASC",
    )
    .bind(marker)
    .fetch_all(&pool)
    .await
    .unwrap();

    let admin_actor = admin();
    let (first, second) = tokio::join!(
        write.approve_edit_proposal(&admin_actor, proposals[0]),
        write.approve_edit_proposal(&admin_actor, proposals[1]),
    );
    let successes = [first.is_ok(), second.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1, "同基准两提案只允许一个成功");
    let conflict_result = if first.is_err() { first } else { second };
    match conflict_result.unwrap_err() {
        WriteError::Conflict(message) => assert_eq!(
            message,
            "点位已更新或提案缺少版本信息，请按最新内容重新提交后审核"
        ),
        other => panic!("期望 409 版本冲突，实际 {other:?}"),
    }

    let version: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id = $1")
        .bind(marker)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 4);
    let approved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marker_edit_proposals WHERE marker_id = $1 AND status = 'APPROVED'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marker_edit_proposals WHERE marker_id = $1 AND status = 'PENDING'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((approved, pending), (1, 1));

    pool.close().await;
}

#[tokio::test]
async fn null_or_stale_base_version_conflict_keeps_pending() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(
        &pool,
        Seed {
            version: 2,
            ..Default::default()
        },
    )
    .await;
    write
        .create_edit_proposal(
            &owner(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("提案".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposal: i64 =
        sqlx::query_scalar("SELECT id FROM marker_edit_proposals WHERE marker_id = $1")
            .bind(marker)
            .fetch_one(&pool)
            .await
            .unwrap();

    sqlx::query("UPDATE marker_edit_proposals SET base_marker_version = NULL WHERE id = $1")
        .bind(proposal)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        write
            .approve_edit_proposal(&admin(), proposal)
            .await
            .unwrap_err(),
        WriteError::Conflict(_)
    ));

    sqlx::query("UPDATE marker_edit_proposals SET base_marker_version = 1, base_content = NULL WHERE id = $1")
        .bind(proposal)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        write
            .approve_edit_proposal(&admin(), proposal)
            .await
            .unwrap_err(),
        WriteError::Conflict(_)
    ));

    // 点位被推进后基准过期。
    sqlx::query("UPDATE marker_edit_proposals SET base_marker_version = 2 WHERE id = $1")
        .bind(proposal)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE map_markers SET version = 3, title = 'Newer content' WHERE id = $1")
        .bind(marker)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        write
            .approve_edit_proposal(&admin(), proposal)
            .await
            .unwrap_err(),
        WriteError::Conflict(_)
    ));

    let status: String =
        sqlx::query_scalar("SELECT status FROM marker_edit_proposals WHERE id = $1")
            .bind(proposal)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "PENDING", "冲突后提案必须仍待审");

    pool.close().await;
}

#[tokio::test]
async fn source_and_translation_edits_share_marker_version() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());
    let marker = seed_marker(
        &pool,
        Seed {
            title: "原文".to_string(),
            description: Some("原描述".to_string()),
            version: 0,
            ..Default::default()
        },
    )
    .await;

    // 译文编辑推进同一 marker.version，且不改动原文文本。
    let after_translation = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("EN title".to_string()),
                description: Some("EN desc".to_string()),
                language: Some("en".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(after_translation.version, 1);
    assert_eq!(after_translation.title, "原文");

    let translation: (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT origin, source_hash, title, description FROM map_marker_translations
         WHERE marker_id = $1 AND language = 'en'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(translation.0, "MANUAL");
    assert_eq!(
        translation.1,
        source_hash_components("zh", Some("原文"), Some("原描述")),
        "人工译文 sourceHash 来自原文"
    );

    // 原文编辑推进同一版本；旧译文靠 hash 失效但不删除、origin 保留。
    let after_source = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("新原文".to_string()),
                description: Some("新描述".to_string()),
                language: Some("zh".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(after_source.version, 2);
    assert_eq!(after_source.title, "新原文");

    let stale = sqlx::query_as::<_, TranslationRow>(
        "SELECT marker_id, language, title, description, source_hash FROM map_marker_translations
         WHERE marker_id = $1 AND language = 'en'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!translation_is_current(&after_source, &stale));
    let (title, _description, content_language) = localize_text(&after_source, Some(&stale), "en");
    assert_eq!(title, "新原文", "过期译文必须回退原文");
    assert_eq!(content_language, "zh");
    let origin: String = sqlx::query_scalar(
        "SELECT origin FROM map_marker_translations WHERE marker_id = $1 AND language = 'en'",
    )
    .bind(marker)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(origin, "MANUAL", "失效译文不得删除或改 origin");

    // 失效后仅补标题（缺描述）应 400；同时给出标题与描述可重建译文。
    assert_bad(
        write
            .admin_update_marker(
                &admin(),
                "zh",
                marker,
                MarkerUpdateRequest {
                    title: Some("EN v2".to_string()),
                    language: Some("en".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err(),
        "该语言尚无有效译文，请同时填写标题和描述（描述可为空）",
    );
    let rebuilt = write
        .admin_update_marker(
            &admin(),
            "zh",
            marker,
            MarkerUpdateRequest {
                title: Some("EN v2".to_string()),
                description: Some(String::new()),
                language: Some("en".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rebuilt.version, 3);

    pool.close().await;
}

#[tokio::test]
async fn chinese_approval_does_not_write_translations_and_manual_sync_remains_atomic() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let write = write_service(
        pool.clone(),
        connect_redis().await,
        &unique_cache_namespace(),
    );
    let marker = seed_marker(&pool, Seed::default()).await;
    write
        .create_edit_proposal(
            &owner(),
            "en",
            marker,
            MarkerUpdateRequest {
                title: Some("中文修改".into()),
                description: Some("中文描述".into()),
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposal = write
        .pending_edit_proposals(&admin())
        .await
        .unwrap()
        .remove(0);
    sqlx::raw_sql("CREATE FUNCTION fail_translation() RETURNS trigger AS $$ BEGIN RAISE EXCEPTION 'translation unavailable'; END; $$ LANGUAGE plpgsql;
        CREATE TRIGGER fail_translation BEFORE INSERT OR UPDATE ON map_marker_translations FOR EACH ROW EXECUTE FUNCTION fail_translation();")
        .execute(&pool).await.unwrap();
    let saved = write
        .approve_edit_proposal(&admin(), proposal.id)
        .await
        .unwrap();
    assert_eq!(saved.title, "中文修改");
    assert_eq!(saved.source_language, "zh");
    assert_eq!(saved.review_status, "APPROVED");
    assert!(
        write
            .admin_update_marker(
                &admin(),
                "en",
                marker,
                MarkerUpdateRequest {
                    title: Some("Manual English".into()),
                    description: Some("English".into()),
                    language: Some("en".into()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let version: i64 = sqlx::query_scalar("SELECT version FROM map_markers WHERE id=$1")
        .bind(marker)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, saved.version, "failed manual sync must roll back");
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_marker_translations WHERE marker_id=$1",
            &[marker]
        )
        .await,
        0
    );
    pool.close().await;
}

#[tokio::test]
async fn created_favorite_and_admin_reads_respect_permissions() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let write = write_service(pool.clone(), redis, &unique_cache_namespace());

    let public = seed_marker(&pool, Seed::default()).await;
    let private = seed_marker(
        &pool,
        Seed {
            is_public: false,
            title: "private".to_string(),
            ..Default::default()
        },
    )
    .await;
    let pending = seed_marker(
        &pool,
        Seed {
            review_status: "PENDING".to_string(),
            title: "pending".to_string(),
            ..Default::default()
        },
    )
    .await;

    let created = write.list_created(&owner()).await.unwrap();
    let ids: Vec<i64> = created.iter().map(|marker| marker.id).collect();
    assert!(ids.contains(&public) && ids.contains(&private) && ids.contains(&pending));
    assert!(write.list_created(&other()).await.unwrap().is_empty());

    write.add_favorite(&owner(), public).await.unwrap();
    write.add_favorite(&owner(), private).await.unwrap();
    write.add_favorite(&owner(), pending).await.unwrap();
    write.add_favorite(&other(), public).await.unwrap();

    assert_eq!(
        write.favorite_ids(&owner()).await.unwrap(),
        vec![public, private, pending],
        "属主可见全部收藏（含私有/待审）"
    );
    assert_eq!(
        write.favorite_ids(&other()).await.unwrap(),
        vec![public],
        "非属主只看公开已审核收藏"
    );
    assert_eq!(write.favorite_markers(&other()).await.unwrap().len(), 1);

    assert!(matches!(
        write.list_all_markers(&other()).await.unwrap_err(),
        WriteError::Forbidden
    ));
    assert_eq!(write.list_all_markers(&admin()).await.unwrap().len(), 3);

    pool.close().await;
}

#[tokio::test]
async fn cache_first_invalidation_and_redis_failure_still_commit() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let redis = connect_redis().await;
    let namespace = unique_cache_namespace();
    let write = write_service(pool.clone(), redis.clone(), &namespace);
    let cache = MarkerCache::new(redis.clone(), true, &namespace);

    assert_eq!(cache.current_generation().await, Some(0));
    let mut req = valid_create();
    req.client_request_id = Some("cache-key".to_string());
    let row = write
        .create_marker(&owner(), "zh", req.clone())
        .await
        .unwrap();
    assert_eq!(
        cache.current_generation().await,
        Some(1),
        "首次失效应从 0 切换为 1"
    );

    // 幂等重放不改动缓存代次。
    let replay = write.create_marker(&owner(), "zh", req).await.unwrap();
    assert_eq!(replay.id, row.id);
    assert_eq!(cache.current_generation().await, Some(1));

    // Redis 不可用：写入仍提交，且耗时有界。
    let failing = MarkerWriteService::new(
        pool.clone(),
        MarkerCache::new(unreachable_redis(), true, unique_cache_namespace()),
    );
    let started = Instant::now();
    let committed = failing
        .create_marker(&owner(), "zh", valid_create())
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "Redis 故障时写入耗时有界，实际 {:?}",
        started.elapsed()
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM map_markers WHERE id = $1",
            &[committed.id]
        )
        .await,
        1,
        "缓存故障不得回滚已提交写入"
    );

    // 禁用缓存时立即返回、不访问 Redis。
    let disabled = MarkerWriteService::new(
        pool.clone(),
        MarkerCache::new(unreachable_redis(), false, unique_cache_namespace()),
    );
    let started = Instant::now();
    disabled
        .create_marker(&owner(), "zh", valid_create())
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));

    let _: i64 = redis.del(format!("{namespace}:gen")).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn image_and_review_changes_do_not_invalidate_content_but_content_changes_do() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let write = write_service(
        pool.clone(),
        connect_redis().await,
        &unique_cache_namespace(),
    );
    let mut request = valid_create();
    request.language = Some("en".into());
    let marker = write.create_marker(&owner(), "en", request).await.unwrap();
    assert_eq!(marker.source_language, "zh");
    write
        .create_edit_proposal(
            &owner(),
            "en",
            marker.id,
            MarkerUpdateRequest {
                title: Some("修改内容".into()),
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let proposal = write
        .pending_edit_proposals(&admin())
        .await
        .unwrap()
        .remove(0);
    write.approve_marker(&admin(), marker.id).await.unwrap();
    sqlx::query("UPDATE map_markers SET mark_image='/uploads/markers/test.jpg', version=version+1 WHERE id=$1")
        .bind(marker.id).execute(&pool).await.unwrap();
    let approved = write
        .approve_edit_proposal(&admin(), proposal.id)
        .await
        .unwrap();
    assert_eq!(approved.title, "修改内容");
    assert_eq!(
        approved.mark_image.as_deref(),
        Some("/uploads/markers/test.jpg")
    );
    write
        .create_edit_proposal(
            &owner(),
            "en",
            marker.id,
            MarkerUpdateRequest {
                title: Some("旧提案".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let stale = write
        .pending_edit_proposals(&admin())
        .await
        .unwrap()
        .remove(0);
    write
        .admin_update_marker(
            &admin(),
            "zh",
            marker.id,
            MarkerUpdateRequest {
                title: Some("较新的内容".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        write.approve_edit_proposal(&admin(), stale.id).await,
        Err(WriteError::Conflict(_))
    ));
    pool.close().await;
}

#[tokio::test]
async fn upgrade_backfills_only_current_proposals_and_retains_manual_english() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    let previous = sqlx::migrate::Migrator::with_migrations(
        lycoris_backend::migrate::MIGRATOR
            .iter()
            .filter(|m| m.version < 8)
            .cloned()
            .collect(),
    );
    previous.run(&pool).await.unwrap();
    let marker = seed_marker(
        &pool,
        Seed {
            lat: 31.2304,
            lng: 121.4737,
            source_language: "en".into(),
            review_status: "PENDING".into(),
            version: 2,
            open_time_start: Some("09:00".into()),
            open_time_end: Some("17:00".into()),
            ..Default::default()
        },
    )
    .await;
    let published = seed_marker(
        &pool,
        Seed {
            source_language: "en".into(),
            ..Default::default()
        },
    )
    .await;
    seed_translation(&pool, published, "zh", "现有译文", None, "test-hash").await;
    for version in [2, 1] {
        sqlx::query(
            "INSERT INTO marker_edit_proposals (marker_id, marker_title, marker_lat, marker_lng,
            proposer_username, proposer_public_id, proposer_is_owner, category, title, language,
            is_public, is_active, base_marker_version, status, created_at)
            SELECT id, title, lat, lng, 'owner', user_public_id, true, category, '中文修改', 'en',
            is_public, is_active, $2, 'PENDING', now() FROM map_markers WHERE id = $1",
        )
        .bind(marker)
        .bind(version as i64)
        .execute(&pool)
        .await
        .unwrap();
    }
    lycoris_backend::migrate::run(&pool).await.unwrap();
    let languages: Vec<String> =
        sqlx::query_scalar("SELECT source_language FROM map_markers ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(languages, vec!["zh", "en"]);
    let proposals: Vec<(i64, String, Option<serde_json::Value>)> =
        sqlx::query_as("SELECT id, language, base_content FROM marker_edit_proposals ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(proposals[0].1, "zh");
    assert!(proposals[0].2.is_some());
    assert!(proposals[1].2.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM map_marker_translations")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    // A photo-only version change must still match the SQL-backfilled Rust snapshot.
    sqlx::query("UPDATE map_markers SET mark_image = '/uploads/synthetic.webp', version = version + 1 WHERE id = $1")
        .bind(marker).execute(&pool).await.unwrap();
    let write = write_service(
        pool.clone(),
        connect_redis().await,
        &unique_cache_namespace(),
    );
    write
        .approve_edit_proposal(&admin(), proposals[0].0)
        .await
        .unwrap();
    assert!(matches!(
        write.approve_edit_proposal(&admin(), proposals[1].0).await,
        Err(WriteError::Conflict(_))
    ));
    pool.close().await;
}
