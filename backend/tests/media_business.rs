//! 媒体业务（头像、受控 `/uploads` 读取、图片提案、清理）真实 PG / Redis / 临时文件测试。
//!
//! 覆盖：头像条件更新不覆盖资料列、已删除与过期行版本；匿名/属主/他人/管理员/提案作者
//! 对公开/私有/已删除点位的访问矩阵（历史 `REJECTED` 提案仍按原规则、已删除关联点位不
//! 授权任何人）；提交提案不直接换图；双管理员审批只能一次成功；审批整体回滚；清理的
//! 存在/缺失/非普通文件与新 URL 竞争 CAS；Redis 失效故障不反转成功；文件异常不误删。
//!
//! 每个用例使用独立临时库与临时上传根目录，退出时自动清理；只使用合成数据。

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{TempDatabase, connect_redis, unique_cache_namespace, unreachable_redis};
use image::{ExtendedColorType, ImageEncoder};
use lycoris_backend::media::{
    AvatarUpdateOutcome, ImageStore, MAX_UPLOAD_BYTES, MediaService, MediaServiceError,
};
use lycoris_backend::modules::markers::cache::MarkerCache;
use lycoris_backend::modules::markers::model::Viewer;
use sqlx::PgPool;
use tempfile::TempDir;
use uuid::Uuid;

fn rgba_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([x as u8, y as u8, 200, 255])
    });
    let mut output = Vec::new();
    image::codecs::png::PngEncoder::new(&mut output)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
        .expect("编码测试 PNG 失败");
    output
}

fn write_media(root: &Path, directory: &str, filename: &str, bytes: &[u8]) {
    let dir = root.join(directory);
    std::fs::create_dir_all(&dir).expect("创建媒体目录失败");
    std::fs::write(dir.join(filename), bytes).expect("写入测试媒体失败");
}

fn markers_dir(root: &Path) -> std::path::PathBuf {
    root.join("markers")
}

fn viewer<'a>(public_id: Option<&'a str>, role: &'a str, deleted: bool) -> Viewer<'a> {
    Viewer {
        public_id,
        role,
        deleted,
    }
}

fn media_service(pool: PgPool, store: ImageStore, redis: fred::clients::Client) -> MediaService {
    MediaService::new(
        pool,
        store,
        MarkerCache::new(redis, true, unique_cache_namespace()),
    )
}

async fn insert_user(
    pool: &PgPool,
    public_id: Uuid,
    username: &str,
    role: &str,
    deleted: bool,
    avatar_url: Option<&str>,
) -> i32 {
    sqlx::query_scalar::<_, i32>(
        "INSERT INTO users
            (public_id, username, nickname, email, password, role, deleted, session_version, row_version, avatar_url)
         VALUES ($1, $2, $3, $4, 'x', $5, $6, 0, 0, $7)
         RETURNING id",
    )
    .bind(public_id)
    .bind(username)
    .bind(username)
    .bind(format!("{username}@example.com"))
    .bind(role)
    .bind(deleted)
    .bind(avatar_url)
    .fetch_one(pool)
    .await
    .expect("插入用户失败")
}

async fn insert_marker(
    pool: &PgPool,
    title: &str,
    is_public: bool,
    review_status: &str,
    owner_public_id: &str,
    mark_image: Option<&str>,
    version: i64,
) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO map_markers (
            lat, lng, category, title, description, source_language, is_public, is_active,
            open_time_start, open_time_end, review_status, username, user_public_id,
            mark_image, last_edited_by_owner, version, created_at, updated_at)
         VALUES (0, 0, 'accessible_toilet', $1, NULL, 'zh', $2, true,
                 NULL, NULL, $3, 'seed-user', $4, $5, true, $6, now(), now())
         RETURNING id",
    )
    .bind(title)
    .bind(is_public)
    .bind(review_status)
    .bind(owner_public_id)
    .bind(mark_image)
    .bind(version)
    .fetch_one(pool)
    .await
    .expect("插入点位失败")
}

async fn insert_proposal(
    pool: &PgPool,
    marker_id: i64,
    marker_title: &str,
    proposer_username: &str,
    proposer_public_id: Option<&str>,
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
    .fetch_one(pool)
    .await
    .expect("插入图片提案失败")
}

async fn proposal_state(pool: &PgPool, proposal_id: i64) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, reviewed_by FROM marker_image_proposals WHERE id = $1")
        .bind(proposal_id)
        .fetch_one(pool)
        .await
        .expect("读取提案状态失败")
}

async fn marker_image_state(pool: &PgPool, marker_id: i64) -> (i64, Option<String>) {
    sqlx::query_as("SELECT version, mark_image FROM map_markers WHERE id = $1")
        .bind(marker_id)
        .fetch_one(pool)
        .await
        .expect("读取点位图片状态失败")
}

#[tokio::test]
async fn avatar_query_and_conditional_update_do_not_touch_profile_or_delete_files() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let root = store.root().to_path_buf();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);

    let public_id = Uuid::new_v4();
    let user_id = insert_user(
        &pool,
        public_id,
        "alice",
        "USER",
        false,
        Some("/uploads/avatars/old.png"),
    )
    .await;
    sqlx::query(
        "UPDATE users SET nickname = 'Nick', pronouns = 'she', signature = 'sig',
                          email = 'alice@example.com' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    write_media(&root, "avatars", "old.png", b"old-avatar");

    // 公共 ID 查询：非法 UUID / 非法存储路径按无头像处理。
    assert_eq!(
        service
            .avatar_url_by_public_id(&public_id.to_string())
            .await
            .unwrap(),
        Some("/uploads/avatars/old.png".to_string())
    );
    assert_eq!(
        service.avatar_url_by_public_id("not-a-uuid").await.unwrap(),
        None
    );
    assert_eq!(
        service.avatar_url_by_user_id(user_id).await.unwrap(),
        Some("/uploads/avatars/old.png".to_string())
    );
    sqlx::query("UPDATE users SET avatar_url = 'http://evil.example/x.png' WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        service
            .avatar_url_by_public_id(&public_id.to_string())
            .await
            .unwrap(),
        None,
        "非 /uploads/avatars/ 路径不得作为头像"
    );
    sqlx::query("UPDATE users SET avatar_url = '/uploads/avatars/old.png' WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    let mut before_success: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    ) = sqlx::query_as(
        "SELECT nickname, pronouns, signature, email, avatar_url, row_version FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    before_success.4 = None; // 仅比较资料列与版本，头像另断言。

    let outcome = service
        .upload_avatar(user_id, 0, &public_id.to_string(), rgba_png(3, 2))
        .await
        .unwrap();
    let new_url = match outcome {
        AvatarUpdateOutcome::Updated {
            avatar_url,
            row_version,
        } => {
            assert_eq!(row_version, 1);
            assert!(
                avatar_url.starts_with("/uploads/avatars/avatar-"),
                "新头像 URL 应带服务端前缀，实际 {avatar_url}"
            );
            avatar_url
        }
        other => panic!("期望 Updated，实际 {other:?}"),
    };

    let after_success: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    ) = sqlx::query_as(
        "SELECT nickname, pronouns, signature, email, avatar_url, row_version FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_success.0, before_success.0);
    assert_eq!(after_success.1, before_success.1);
    assert_eq!(after_success.2, before_success.2);
    assert_eq!(after_success.3, before_success.3);
    assert_eq!(after_success.5, 1, "row_version 应推进");
    assert_eq!(after_success.4.as_deref(), Some(new_url.as_str()));

    // 不自动删除旧头像文件。
    assert!(
        root.join("avatars").join("old.png").is_file(),
        "旧头像文件必须保留"
    );
    let new_filename = new_url.strip_prefix("/uploads/avatars/").unwrap();
    assert!(root.join("avatars").join(new_filename).is_file());

    // 过期行版本：拒绝且不误报成功、不覆盖头像、不新增文件。
    let files_before = std::fs::read_dir(root.join("avatars"))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    let outcome = service
        .upload_avatar(user_id, 0, &public_id.to_string(), rgba_png(2, 2))
        .await
        .unwrap();
    assert_eq!(outcome, AvatarUpdateOutcome::VersionConflict);
    let stored_after_conflict: Option<String> =
        sqlx::query_scalar("SELECT avatar_url FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_after_conflict.as_deref(), Some(new_url.as_str()));
    let files_after = std::fs::read_dir(root.join("avatars"))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    assert_eq!(files_before, files_after, "版本冲突不得落盘新文件");

    // 已删除用户：拒绝，不落盘。
    sqlx::query("UPDATE users SET deleted = true WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let outcome = service
        .upload_avatar(user_id, 1, &public_id.to_string(), rgba_png(2, 2))
        .await
        .unwrap();
    assert_eq!(outcome, AvatarUpdateOutcome::NotFound);
    assert_eq!(
        service.avatar_url_by_user_id(user_id).await.unwrap(),
        None,
        "已删除用户查询按无头像处理"
    );
}

#[tokio::test]
async fn marker_media_access_matrix_covers_anonymous_owner_other_admin_and_proposer() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let root = store.root().to_path_buf();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);

    write_media(&root, "markers", "direct.png", b"direct");
    write_media(&root, "markers", "private.png", b"private");
    write_media(&root, "markers", "proposal.png", b"proposal");
    write_media(&root, "markers", "deleted.png", b"deleted");
    write_media(&root, "avatars", "a.png", b"avatar");

    let owner = "11111111-1111-1111-1111-111111111111";
    let other = "22222222-2222-2222-2222-222222222222";
    let admin = "33333333-3333-3333-3333-333333333333";
    let proposer = "44444444-4444-4444-4444-444444444444";

    insert_marker(
        &pool,
        "公开直接引用",
        true,
        "APPROVED",
        owner,
        Some("/uploads/markers/direct.png"),
        0,
    )
    .await;
    insert_marker(
        &pool,
        "私有属主",
        false,
        "APPROVED",
        owner,
        Some("/uploads/markers/private.png"),
        0,
    )
    .await;
    let proposal_marker =
        insert_marker(&pool, "提案公开点位", true, "APPROVED", owner, None, 0).await;
    insert_proposal(
        &pool,
        proposal_marker,
        "提案公开点位",
        "proposer-name",
        Some(proposer),
        "/uploads/markers/proposal.png",
        "REJECTED",
    )
    .await;
    let deleted_marker = insert_marker(&pool, "将被删除", true, "APPROVED", owner, None, 0).await;
    insert_proposal(
        &pool,
        deleted_marker,
        "将被删除",
        "proposer-name",
        Some(proposer),
        "/uploads/markers/deleted.png",
        "PENDING",
    )
    .await;
    sqlx::query("DELETE FROM map_markers WHERE id = $1")
        .bind(deleted_marker)
        .execute(&pool)
        .await
        .unwrap();

    let owner_viewer = viewer(Some(owner), "USER", false);
    let other_viewer = viewer(Some(other), "USER", false);
    let admin_viewer = viewer(Some(admin), "ADMIN", false);
    let proposer_viewer = viewer(Some(proposer), "USER", false);

    // avatars 匿名可读；非法目录/文件名按 404。
    assert!(service.open_uploads("avatars", "a.png", None).await.is_ok());
    assert!(matches!(
        service.open_uploads("etc", "a.png", None).await,
        Err(MediaServiceError::NotFound(_))
    ));
    assert!(matches!(
        service.open_uploads("markers", "../a.png", None).await,
        Err(MediaServiceError::NotFound(_))
    ));

    // 直接引用的公开点位：所有 viewer 均可读。
    for (label, candidate) in [
        ("anon", None),
        ("owner", Some(&owner_viewer)),
        ("other", Some(&other_viewer)),
        ("admin", Some(&admin_viewer)),
        ("proposer", Some(&proposer_viewer)),
    ] {
        assert!(
            service
                .open_uploads("markers", "direct.png", candidate)
                .await
                .is_ok(),
            "公开直接引用应对 {label} 可读"
        );
    }

    // 私有直接引用：仅属主与管理员。
    assert!(matches!(
        service.open_uploads("markers", "private.png", None).await,
        Err(MediaServiceError::NotFound(_))
    ));
    assert!(
        service
            .open_uploads("markers", "private.png", Some(&owner_viewer))
            .await
            .is_ok()
    );
    assert!(matches!(
        service
            .open_uploads("markers", "private.png", Some(&other_viewer))
            .await,
        Err(MediaServiceError::NotFound(_))
    ));
    assert!(
        service
            .open_uploads("markers", "private.png", Some(&admin_viewer))
            .await
            .is_ok()
    );
    assert!(matches!(
        service
            .open_uploads("markers", "private.png", Some(&proposer_viewer))
            .await,
        Err(MediaServiceError::NotFound(_))
    ));

    // 历史 REJECTED 提案：状态不参与判断，公开点位的提案作者可读，他人不可读。
    assert!(
        service
            .open_uploads("markers", "proposal.png", Some(&proposer_viewer))
            .await
            .is_ok(),
        "提案状态不是可读条件"
    );
    assert!(matches!(
        service
            .open_uploads("markers", "proposal.png", Some(&other_viewer))
            .await,
        Err(MediaServiceError::NotFound(_))
    ));
    assert!(
        service
            .open_uploads("markers", "proposal.png", Some(&admin_viewer))
            .await
            .is_ok()
    );
    assert!(matches!(
        service.open_uploads("markers", "proposal.png", None).await,
        Err(MediaServiceError::NotFound(_))
    ));

    // 关联点位已删除：历史提案不授权任何人，包括管理员。
    for candidate in [Some(&admin_viewer), Some(&proposer_viewer)] {
        assert!(matches!(
            service
                .open_uploads("markers", "deleted.png", candidate)
                .await,
            Err(MediaServiceError::NotFound(_))
        ));
    }

    // 已删除身份只能与匿名同权：旧 ADMIN/属主/提案作者身份一律不算数。
    let deleted_admin_viewer = viewer(Some(admin), "ADMIN", true);
    let deleted_owner_viewer = viewer(Some(owner), "USER", true);
    let deleted_proposer_viewer = viewer(Some(proposer), "USER", true);

    // 公开直接引用与匿名一致，仍可读。
    for (label, candidate) in [
        ("deleted-admin", Some(&deleted_admin_viewer)),
        ("deleted-owner", Some(&deleted_owner_viewer)),
        ("deleted-proposer", Some(&deleted_proposer_viewer)),
    ] {
        assert!(
            service
                .open_uploads("markers", "direct.png", candidate)
                .await
                .is_ok(),
            "已删除 {label} 对公开直接引用应保持匿名同权可读"
        );
    }

    // 私有直接引用：已删除的旧属主/管理员都不可读。
    for (label, candidate) in [
        ("deleted-admin", Some(&deleted_admin_viewer)),
        ("deleted-owner", Some(&deleted_owner_viewer)),
    ] {
        assert!(
            matches!(
                service
                    .open_uploads("markers", "private.png", candidate)
                    .await,
                Err(MediaServiceError::NotFound(_))
            ),
            "已删除 {label} 不得读取私有直接引用"
        );
    }

    // 历史提案：已删除的旧提案作者/管理员都不可读。
    for (label, candidate) in [
        ("deleted-admin", Some(&deleted_admin_viewer)),
        ("deleted-proposer", Some(&deleted_proposer_viewer)),
    ] {
        assert!(
            matches!(
                service
                    .open_uploads("markers", "proposal.png", candidate)
                    .await,
                Err(MediaServiceError::NotFound(_))
            ),
            "已删除 {label} 不得凭旧提案身份读取私有图片"
        );
    }
}

#[tokio::test]
async fn submit_marker_image_inserts_pending_proposal_without_changing_marker() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let root = store.root().to_path_buf();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);

    let owner = "11111111-1111-1111-1111-111111111111";
    let marker_id = insert_marker(
        &pool,
        "提案标题快照",
        true,
        "APPROVED",
        owner,
        Some("/uploads/markers/original.png"),
        3,
    )
    .await;
    let owner_viewer = viewer(Some(owner), "USER", false);

    let locked = service
        .submit_marker_image(
            marker_id,
            Some(&owner_viewer),
            "owner-name",
            owner,
            rgba_png(3, 3),
        )
        .await
        .unwrap();
    assert_eq!(locked.id, marker_id);
    assert_eq!(locked.title, "提案标题快照");
    assert_eq!(locked.version, 3, "提交不得推进点位版本");

    let (proposal_status, proposal_title, proposal_username, proposal_public_id, image_url): (
        String,
        String,
        String,
        Option<String>,
        String,
    ) = sqlx::query_as(
        "SELECT status, marker_title, proposer_username, proposer_public_id, image_url
         FROM marker_image_proposals WHERE marker_id = $1",
    )
    .bind(marker_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(proposal_status, "PENDING");
    assert_eq!(proposal_title, "提案标题快照");
    assert_eq!(proposal_username, "owner-name");
    assert_eq!(proposal_public_id.as_deref(), Some(owner));
    assert!(
        image_url.starts_with("/uploads/markers/proposal-marker-"),
        "实际 URL：{image_url}"
    );
    let filename = image_url.strip_prefix("/uploads/markers/").unwrap();
    assert!(markers_dir(&root).join(filename).is_file());

    // 点位本身未被改动。
    let (version, mark_image) = marker_image_state(&pool, marker_id).await;
    assert_eq!(version, 3);
    assert_eq!(
        mark_image.as_deref(),
        Some("/uploads/markers/original.png"),
        "提交提案不得直接换图"
    );

    // 不可见点位：可见性检查在解码/落盘之前，不产生新文件。
    let private = insert_marker(&pool, "私有", false, "APPROVED", owner, None, 0).await;
    let files_before = std::fs::read_dir(markers_dir(&root))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    let error = service
        .submit_marker_image(
            private,
            None,
            "someone",
            "55555555-5555-5555-5555-555555555555",
            rgba_png(2, 2),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::NOT_FOUND);
    let files_after = std::fs::read_dir(markers_dir(&root))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    assert_eq!(files_before, files_after, "不可见点位不得解码或落盘新文件");

    // 空文件是 400，且不落盘。
    let error = service
        .submit_marker_image(
            marker_id,
            Some(&owner_viewer),
            "owner-name",
            owner,
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);

    // 超过 5 MiB：由 ImageStore 边界判定，映射为 413，HTTP 可经显式分支识别（不靠字符串）。
    let error = service
        .submit_marker_image(
            marker_id,
            Some(&owner_viewer),
            "owner-name",
            owner,
            vec![0u8; MAX_UPLOAD_BYTES + 1],
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert!(error.is_payload_too_large());
    assert_eq!(error.message(), "上传文件过大，请选择 5MB 以内的图片");

    // 非法/损坏图片输入是 400（不是 500）。
    let error = service
        .submit_marker_image(
            marker_id,
            Some(&owner_viewer),
            "owner-name",
            owner,
            b"not an image".to_vec(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn image_proposal_review_is_one_shot_and_reject_does_not_require_marker() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);

    let owner = "11111111-1111-1111-1111-111111111111";
    let admin = "33333333-3333-3333-3333-333333333333";
    let other = "22222222-2222-2222-2222-222222222222";
    let admin_viewer = viewer(Some(admin), "ADMIN", false);
    let user_viewer = viewer(Some(other), "USER", false);

    let marker_id = insert_marker(&pool, "待审图", true, "APPROVED", owner, None, 0).await;
    let proposal = insert_proposal(
        &pool,
        marker_id,
        "待审图",
        "proposer-name",
        Some(owner),
        "/uploads/markers/new.png",
        "PENDING",
    )
    .await;

    // 非管理员被拒。
    assert_eq!(
        service
            .approve_image_proposal(proposal, &user_viewer, "other")
            .await
            .unwrap_err()
            .status(),
        axum::http::StatusCode::FORBIDDEN
    );

    let updated = service
        .approve_image_proposal(proposal, &admin_viewer, "admin1")
        .await
        .unwrap();
    assert_eq!(
        updated.mark_image.as_deref(),
        Some("/uploads/markers/new.png")
    );
    assert_eq!(updated.version, 1);
    let (status, reviewer) = proposal_state(&pool, proposal).await;
    assert_eq!(status, "APPROVED");
    assert_eq!(reviewer.as_deref(), Some("admin1"));

    // 第二名管理员/重复处理：400，点位不再推进。
    let error = service
        .approve_image_proposal(proposal, &admin_viewer, "admin2")
        .await
        .unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(marker_image_state(&pool, marker_id).await.0, 1);

    // 不存在的提案：404。
    assert_eq!(
        service
            .approve_image_proposal(9_999_999, &admin_viewer, "admin1")
            .await
            .unwrap_err()
            .status(),
        axum::http::StatusCode::NOT_FOUND
    );

    // 拒绝不要求关联点位存在。
    let orphan = insert_proposal(
        &pool,
        8_888_888,
        "孤儿",
        "proposer-name",
        Some(owner),
        "/uploads/markers/orphan.png",
        "PENDING",
    )
    .await;
    service
        .reject_image_proposal(orphan, &admin_viewer, "admin1")
        .await
        .unwrap();
    let (status, reviewer) = proposal_state(&pool, orphan).await;
    assert_eq!(status, "REJECTED");
    assert_eq!(reviewer.as_deref(), Some("admin1"));
    assert_eq!(
        service
            .reject_image_proposal(orphan, &admin_viewer, "admin1")
            .await
            .unwrap_err()
            .status(),
        axum::http::StatusCode::BAD_REQUEST
    );

    // 待审清单契约：PENDING、createdAt DESC、8 字段。
    let second = insert_proposal(
        &pool,
        marker_id,
        "第二张",
        "proposer-name",
        Some(owner),
        "/uploads/markers/second.png",
        "PENDING",
    )
    .await;
    let pending = service.list_pending_images(&admin_viewer).await.unwrap();
    assert_eq!(pending.len(), 1, "已处理/拒绝的提案不应出现在待审清单");
    assert_eq!(pending[0].id, second);
    assert_eq!(pending[0].marker_id, marker_id);
    assert_eq!(pending[0].marker_title, "第二张");
    assert_eq!(pending[0].proposer_username, "proposer-name");
    assert_eq!(pending[0].proposer_public_id.as_deref(), Some(owner));
    assert_eq!(pending[0].image_url, "/uploads/markers/second.png");
    assert_eq!(pending[0].status, "PENDING");
    let serialized = serde_json::to_value(&pending[0]).unwrap();
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
        assert!(serialized.get(key).is_some(), "清单缺少字段 {key}");
    }
}

#[tokio::test]
async fn approve_rolls_back_marker_when_proposal_update_fails() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);

    let owner = "11111111-1111-1111-1111-111111111111";
    let admin = viewer(Some("33333333-3333-3333-3333-333333333333"), "ADMIN", false);
    let marker_id = insert_marker(
        &pool,
        "原图",
        true,
        "APPROVED",
        owner,
        Some("/uploads/markers/original.png"),
        5,
    )
    .await;
    let proposal = insert_proposal(
        &pool,
        marker_id,
        "原图",
        "proposer-name",
        Some(owner),
        "/uploads/markers/new.png",
        "PENDING",
    )
    .await;

    // 注入提案 UPDATE 触发器失败：审批事务必须整体回滚。
    sqlx::query(
        "CREATE FUNCTION fail_image_proposal_review() RETURNS trigger AS $$
         BEGIN
             RAISE EXCEPTION 'forced proposal update failure';
         END;
         $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER trg_fail_image_proposal_review
         BEFORE UPDATE ON marker_image_proposals
         FOR EACH ROW WHEN (NEW.status = 'APPROVED')
         EXECUTE FUNCTION fail_image_proposal_review()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let error = service
        .approve_image_proposal(proposal, &admin, "admin1")
        .await
        .unwrap_err();
    assert_eq!(
        error.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );

    let (version, mark_image) = marker_image_state(&pool, marker_id).await;
    assert_eq!(version, 5, "审批失败必须回滚点位版本");
    assert_eq!(mark_image.as_deref(), Some("/uploads/markers/original.png"));
    let (status, reviewer) = proposal_state(&pool, proposal).await;
    assert_eq!(status, "PENDING", "审批失败必须回滚提案状态");
    assert!(reviewer.is_none());

    // 清理注入的触发器，便于后续断言（临时库也会销毁）。
    sqlx::query("DROP TRIGGER trg_fail_image_proposal_review ON marker_image_proposals")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn cleanup_clears_only_missing_and_survives_new_url_cas() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let root = store.root().to_path_buf();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis);
    let admin = viewer(Some("33333333-3333-3333-3333-333333333333"), "ADMIN", false);

    write_media(&root, "markers", "present.png", b"present");
    std::fs::create_dir_all(markers_dir(&root).join("folder.png")).unwrap();

    let present = insert_marker(
        &pool,
        "存在",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/present.png"),
        0,
    )
    .await;
    let missing = insert_marker(
        &pool,
        "缺失",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/missing.png"),
        0,
    )
    .await;
    let illegal = insert_marker(
        &pool,
        "非法路径",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/../../secret.png"),
        0,
    )
    .await;
    let blank = insert_marker(
        &pool,
        "空文件名",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/"),
        0,
    )
    .await;
    let folder = insert_marker(
        &pool,
        "非普通文件",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/folder.png"),
        0,
    )
    .await;
    // 非 markers 前缀不参与清理。
    insert_marker(
        &pool,
        "其他目录",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/avatars/whatever.png"),
        0,
    )
    .await;

    let result = service.cleanup_missing_images(&admin).await.unwrap();
    assert_eq!(result.checked, 5, "只统计 /uploads/markers/ 引用");
    assert_eq!(result.cleared, 2, "仅缺失与目录/非普通文件被清空");
    assert_eq!(result.message, "失效图片链接清理完成");

    assert_eq!(
        marker_image_state(&pool, present).await.1.as_deref(),
        Some("/uploads/markers/present.png"),
        "存在的普通文件必须保留"
    );
    assert!(markers_dir(&root).join("present.png").is_file());
    assert_eq!(marker_image_state(&pool, missing).await.1, None);
    assert_eq!(
        marker_image_state(&pool, illegal).await.1.as_deref(),
        Some("/uploads/markers/../../secret.png")
    );
    assert_eq!(
        marker_image_state(&pool, blank).await.1.as_deref(),
        Some("/uploads/markers/")
    );
    assert_eq!(marker_image_state(&pool, folder).await.1, None);
    assert!(
        markers_dir(&root).join("folder.png").is_dir(),
        "清理不得删除文件或目录"
    );

    // 并发换图 CAS：旧检查不得清掉刚换上的新 URL。
    let raced = insert_marker(
        &pool,
        "竞争",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/race-old.png"),
        0,
    )
    .await;
    let mut lock_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM map_markers WHERE id = $1 FOR UPDATE")
        .bind(raced)
        .fetch_one(&mut *lock_tx)
        .await
        .unwrap();

    let raced_service = service.clone();
    let raced_admin = admin;
    let handle =
        tokio::spawn(async move { raced_service.cleanup_missing_images(&raced_admin).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    sqlx::query(
        "UPDATE map_markers SET mark_image = '/uploads/markers/race-new.png', version = version + 1 WHERE id = $1",
    )
    .bind(raced)
    .execute(&mut *lock_tx)
    .await
    .unwrap();
    lock_tx.commit().await.unwrap();

    let raced_result = handle.await.unwrap().unwrap();
    assert_eq!(raced_result.cleared, 0, "旧检查不得清空并发换上的新图");
    let (version, mark_image) = marker_image_state(&pool, raced).await;
    assert_eq!(version, 1);
    assert_eq!(mark_image.as_deref(), Some("/uploads/markers/race-new.png"));
}

#[tokio::test]
async fn approval_succeeds_even_when_cache_invalidation_fails() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    // 指向未监听端口的 Redis：失效必须受限且不影响已提交结果。
    let cache = MarkerCache::new(unreachable_redis(), true, unique_cache_namespace());
    let service = MediaService::new(pool.clone(), store, cache);

    let owner = "11111111-1111-1111-1111-111111111111";
    let admin = viewer(Some("33333333-3333-3333-3333-333333333333"), "ADMIN", false);
    let marker_id = insert_marker(&pool, "缓存故障", true, "APPROVED", owner, None, 0).await;
    let proposal = insert_proposal(
        &pool,
        marker_id,
        "缓存故障",
        "proposer-name",
        Some(owner),
        "/uploads/markers/cached.png",
        "PENDING",
    )
    .await;

    let started = Instant::now();
    let updated = service
        .approve_image_proposal(proposal, &admin, "admin1")
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(updated.version, 1);
    assert!(
        elapsed < Duration::from_secs(2),
        "缓存失效等待必须受 500ms 超时约束，实际 {elapsed:?}"
    );
    let (status, reviewer) = proposal_state(&pool, proposal).await;
    assert_eq!(status, "APPROVED");
    assert_eq!(reviewer.as_deref(), Some("admin1"));
}

#[tokio::test]
async fn cleanup_invalidates_cache_for_committed_clears_when_later_db_fails() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let redis = connect_redis().await;
    let namespace = unique_cache_namespace();
    let service = MediaService::new(
        pool.clone(),
        store,
        MarkerCache::new(redis.clone(), true, &namespace),
    );
    let admin = viewer(Some("33333333-3333-3333-3333-333333333333"), "ADMIN", false);

    // 两个缺失文件引用：id 较小的先处理并成功提交，id 较大的 UPDATE 抛错。
    let first = insert_marker(
        &pool,
        "先清理",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/clean-a.png"),
        0,
    )
    .await;
    let second = insert_marker(
        &pool,
        "后失败",
        true,
        "APPROVED",
        "11111111-1111-1111-1111-111111111111",
        Some("/uploads/markers/clean-b.png"),
        0,
    )
    .await;
    sqlx::query(
        "CREATE FUNCTION fail_second_cleanup() RETURNS trigger AS $$
         BEGIN
             RAISE EXCEPTION 'forced cleanup update failure';
         END;
         $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER trg_fail_second_cleanup
         BEFORE UPDATE ON map_markers
         FOR EACH ROW WHEN (OLD.mark_image = '/uploads/markers/clean-b.png')
         EXECUTE FUNCTION fail_second_cleanup()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let cache = MarkerCache::new(redis.clone(), true, &namespace);
    let generation_before = cache.current_generation().await;

    let error = service
        .cleanup_missing_images(&admin)
        .await
        .expect_err("后续 DB 失败必须返回受控错误");
    assert_eq!(
        error.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );

    // 已提交的清理保留，未提交的失败者不动；不假称全量回滚。
    let (first_version, first_image) = marker_image_state(&pool, first).await;
    assert_eq!(first_image, None, "已提交的清理必须保留");
    assert_eq!(first_version, 1);
    assert_eq!(
        marker_image_state(&pool, second).await.1.as_deref(),
        Some("/uploads/markers/clean-b.png"),
        "失败的清理不得生效"
    );

    // 已提交部分触发了缓存失效。
    let generation_after = cache.current_generation().await;
    assert!(
        generation_after > generation_before,
        "已提交的清理必须触发缓存失效：{generation_before:?} -> {generation_after:?}"
    );
}

#[tokio::test]
async fn soft_deletion_preserves_images_and_proposals_but_revokes_public_and_owner_access() {
    use lycoris_backend::modules::markers::write::MarkerWriteService;
    use lycoris_backend::modules::markers::write_model::Actor;
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let root = store.root().to_path_buf();
    let redis = connect_redis().await;
    let service = media_service(pool.clone(), store, redis.clone());
    let write = MarkerWriteService::new(
        pool.clone(),
        MarkerCache::new(redis, true, unique_cache_namespace()),
    );
    let admin = viewer(Some("admin"), "ADMIN", false);
    let owner = viewer(Some("owner"), "USER", false);
    let actor = Actor::new("admin", "admin", true);
    write_media(&root, "markers", "retained.png", &rgba_png(2, 2));
    write_media(&root, "markers", "proposal.png", &rgba_png(2, 2));
    let marker = insert_marker(
        &pool,
        "retained",
        true,
        "APPROVED",
        "owner",
        Some("/uploads/markers/retained.png"),
        0,
    )
    .await;
    let proposal = insert_proposal(
        &pool,
        marker,
        "retained",
        "owner",
        Some("owner"),
        "/uploads/markers/proposal.png",
        "PENDING",
    )
    .await;
    assert!(
        service
            .open_uploads("markers", "retained.png", None)
            .await
            .is_ok()
    );
    write.admin_delete_marker(&actor, marker).await.unwrap();
    for filename in ["retained.png", "proposal.png"] {
        for candidate in [None, Some(&owner)] {
            assert!(matches!(
                service.open_uploads("markers", filename, candidate).await,
                Err(MediaServiceError::NotFound(_))
            ));
        }
        assert!(
            service
                .open_uploads("markers", filename, Some(&admin))
                .await
                .is_ok()
        );
        assert!(root.join("markers").join(filename).is_file());
    }
    assert!(
        service
            .list_pending_images(&admin)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .approve_image_proposal(proposal, &admin, "admin")
            .await
            .is_err()
    );
    assert_eq!(proposal_state(&pool, proposal).await.0, "PENDING");
    assert_eq!(
        marker_image_state(&pool, marker).await.1.as_deref(),
        Some("/uploads/markers/retained.png")
    );
    // Cleanup must not change even a missing archived reference.
    let archived_missing = insert_marker(
        &pool,
        "missing",
        true,
        "APPROVED",
        "owner",
        Some("/uploads/markers/missing.png"),
        0,
    )
    .await;
    write
        .admin_delete_marker(&actor, archived_missing)
        .await
        .unwrap();
    assert_eq!(
        service
            .cleanup_missing_images(&admin)
            .await
            .unwrap()
            .cleared,
        0
    );
    assert_eq!(
        marker_image_state(&pool, archived_missing)
            .await
            .1
            .as_deref(),
        Some("/uploads/markers/missing.png")
    );
    write.admin_restore_marker(&actor, marker).await.unwrap();
    assert!(
        service
            .open_uploads("markers", "retained.png", None)
            .await
            .is_ok()
    );
    assert_eq!(service.list_pending_images(&admin).await.unwrap().len(), 1);
    pool.close().await;
}

#[tokio::test]
async fn album_retains_cover_orders_by_submission_and_guards_every_photo() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    let dir = TempDir::new().unwrap();
    let store = ImageStore::with_default_concurrency(dir.path()).unwrap();
    let service = media_service(pool.clone(), store, connect_redis().await);
    let owner = "11111111-1111-1111-1111-111111111111";
    let admin = viewer(Some("33333333-3333-3333-3333-333333333333"), "ADMIN", false);
    let marker = insert_marker(&pool, "Album", true, "APPROVED", owner, None, 0).await;
    let first = insert_proposal(
        &pool,
        marker,
        "Album",
        "owner",
        Some(owner),
        "/uploads/markers/first.png",
        "PENDING",
    )
    .await;
    let second = insert_proposal(
        &pool,
        marker,
        "Album",
        "owner",
        Some(owner),
        "/uploads/markers/second.png",
        "PENDING",
    )
    .await;
    for name in ["first.png", "second.png"] {
        write_media(dir.path(), "markers", name, &rgba_png(2, 2));
    }
    // Reviewing in reverse must not reverse the contribution's chosen cover.
    service
        .approve_image_proposal(second, &admin, "admin")
        .await
        .unwrap();
    service
        .approve_image_proposal(first, &admin, "admin")
        .await
        .unwrap();
    assert_eq!(
        marker_image_state(&pool, marker).await.1.as_deref(),
        Some("/uploads/markers/first.png")
    );
    let urls: Vec<String> = sqlx::query_scalar(
        "SELECT image_url FROM marker_photos WHERE marker_id=$1 ORDER BY sort_order,id",
    )
    .bind(marker)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        urls,
        ["/uploads/markers/first.png", "/uploads/markers/second.png"]
    );
    assert!(
        service
            .approve_image_proposal(first, &admin, "admin")
            .await
            .is_err()
    );
    for name in ["first.png", "second.png"] {
        assert!(service.open_uploads("markers", name, None).await.is_ok());
    }
    sqlx::query("UPDATE map_markers SET is_public=false WHERE id=$1")
        .bind(marker)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        service
            .open_uploads("markers", "second.png", None)
            .await
            .is_err()
    );
    let owner_viewer = viewer(Some(owner), "USER", false);
    assert!(
        service
            .open_uploads("markers", "second.png", Some(&owner_viewer))
            .await
            .is_ok()
    );
    sqlx::query("UPDATE map_markers SET deactivated=true WHERE id=$1")
        .bind(marker)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        service
            .open_uploads("markers", "second.png", Some(&owner_viewer))
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM marker_photos WHERE marker_id=$1")
        .bind(marker)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "soft deletion must retain the album");
}
