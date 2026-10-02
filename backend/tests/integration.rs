//! 真实 PG / Redis 集成测试（阶段 1 基础工程）。
//!
//! 共享工具见 `tests/common/mod.rs`。每个用例创建并清理自己的 UUID 临时库；
//! 服务不可用时直接失败，不做静默跳过。

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{
    BASELINE_TABLES, TempDatabase, UNREACHABLE_PG_URL, UNREACHABLE_REDIS_URL, connect_redis,
    test_redis_url, unreachable_redis,
};
use lycoris_backend::app::{AppState, build_router};
use lycoris_backend::config::Config;
use lycoris_backend::migrate::{self, MIGRATOR, MigrationError};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

#[tokio::test]
async fn migrates_baseline_and_passes_health_checks() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    migrate::run(&pool).await.expect("执行基线迁移失败");
    migrate::verify_applied(&pool)
        .await
        .expect("已迁移库的校验应通过");

    for table in BASELINE_TABLES {
        let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("查询表是否存在失败");
        assert!(exists.is_some(), "迁移后缺少表 {table}，基线未正确应用");
    }

    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("查询迁移记录失败");
    // 完整迁移应登记内嵌迁移集合的全部版本；0005 之后即为 [1, 2, 3, 4, 5]。
    let expected: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
    assert_eq!(applied, expected, "完整迁移应登记内嵌迁移的全部版本");
    assert_eq!(
        applied,
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        "0011 之后完整迁移版本应为 [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]"
    );

    let redis = connect_redis().await;
    let mut config = Config::new(temp.url(), test_redis_url());
    let _upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
    config.upload_dir = _upload.path().to_path_buf();
    let router =
        build_router(AppState::new(pool.clone(), redis, config).expect("构造 AppState 失败"));

    let live = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("调用 /health/live 失败");
    assert_eq!(live.status(), StatusCode::OK);

    let ready = router
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("调用 /health/ready 失败");
    assert_eq!(
        ready.status(),
        StatusCode::OK,
        "/health/ready 应在 PG 与 Redis 可用时返回 200"
    );

    pool.close().await;
}

#[tokio::test]
async fn ready_reports_503_when_database_unavailable() {
    // 懒加载连接池指向未监听端口，acquire 很快超时；不停止任何共享服务。
    let bad_db = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy(UNREACHABLE_PG_URL)
        .expect("构造坏 PG 连接池失败");

    let redis = connect_redis().await;
    let mut config = Config::new(UNREACHABLE_PG_URL, test_redis_url());
    let _upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
    config.upload_dir = _upload.path().to_path_buf();
    let router = build_router(AppState::new(bad_db, redis, config).expect("构造 AppState 失败"));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("调用 /health/ready 失败");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "PG 不可用时 /health/ready 必须返回 503"
    );
}

#[tokio::test]
async fn ready_reports_503_when_redis_unavailable() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    migrate::run(&pool).await.expect("执行基线迁移失败");

    let redis = unreachable_redis();
    let mut config = Config::new(temp.url(), UNREACHABLE_REDIS_URL);
    let _upload = tempfile::TempDir::new().expect("创建临时上传目录失败");
    config.upload_dir = _upload.path().to_path_buf();
    let router =
        build_router(AppState::new(pool.clone(), redis, config).expect("构造 AppState 失败"));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("调用 /health/ready 失败");
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "Redis 不可用时 /health/ready 必须返回 503"
    );

    pool.close().await;
}

#[tokio::test]
async fn verify_rejects_unmigrated_database_without_ddl() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    let error = migrate::verify_applied(&pool)
        .await
        .expect_err("未迁移的空库必须被拒绝");
    assert!(
        matches!(error, MigrationError::NotMigrated),
        "期望 NotMigrated，实际为 {error}"
    );

    // 只读校验不得创建任何表（尤其是 _sqlx_migrations）。
    let present: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations')::text")
            .fetch_one(&pool)
            .await
            .expect("查询迁移表失败");
    assert!(present.is_none(), "只读校验不得创建 _sqlx_migrations 表");

    pool.close().await;
}

#[tokio::test]
async fn verify_rejects_dropped_migration_record() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    migrate::run(&pool).await.expect("执行基线迁移失败");
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("删除迁移记录失败");

    let error = migrate::verify_applied(&pool)
        .await
        .expect_err("迁移记录缺失必须被拒绝");
    assert!(
        matches!(error, MigrationError::Missing(1)),
        "期望 Missing(1)，实际为 {error}"
    );

    pool.close().await;
}

#[tokio::test]
async fn verify_rejects_checksum_mismatch() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    migrate::run(&pool).await.expect("执行基线迁移失败");
    sqlx::query(
        "UPDATE _sqlx_migrations SET checksum = decode('deadbeef', 'hex') WHERE version = 1",
    )
    .execute(&pool)
    .await
    .expect("篡改校验和失败");

    let error = migrate::verify_applied(&pool)
        .await
        .expect_err("校验和不一致必须被拒绝");
    assert!(
        matches!(error, MigrationError::ChecksumMismatch(1)),
        "期望 ChecksumMismatch(1)，实际为 {error}"
    );

    pool.close().await;
}

#[tokio::test]
async fn verify_rejects_failed_migration_record() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    migrate::run(&pool).await.expect("执行基线迁移失败");
    sqlx::query("UPDATE _sqlx_migrations SET success = false WHERE version = 1")
        .execute(&pool)
        .await
        .expect("标记迁移失败状态失败");

    let error = migrate::verify_applied(&pool)
        .await
        .expect_err("未完成的迁移记录必须被拒绝");
    assert!(
        matches!(error, MigrationError::Failed(1)),
        "期望 Failed(1)，实际为 {error}"
    );

    pool.close().await;
}

#[tokio::test]
async fn verify_rejects_unknown_applied_migration() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;

    migrate::run(&pool).await.expect("执行基线迁移失败");
    sqlx::query(
        "INSERT INTO _sqlx_migrations \
         (version, description, installed_on, success, checksum, execution_time) \
         VALUES (999, 'unknown', now(), true, decode('00', 'hex'), 0)",
    )
    .execute(&pool)
    .await
    .expect("写入未知迁移记录失败");

    let error = migrate::verify_applied(&pool)
        .await
        .expect_err("未知的已应用迁移必须被拒绝");
    assert!(
        matches!(error, MigrationError::UnknownApplied(999)),
        "期望 UnknownApplied(999)，实际为 {error}"
    );

    pool.close().await;
}

#[tokio::test]
async fn migrate_cli_applies_baseline_to_empty_database() {
    let temp = TempDatabase::create().await;

    // 直接执行编译出的 bin，覆盖 main() 的 --migrate 参数装配。
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lycoris-backend"))
        .arg("--migrate")
        .env("DATABASE_URL", temp.url())
        .env("REDIS_URL", test_redis_url())
        .output()
        .expect("启动 lycoris-backend --migrate 失败");
    assert!(
        output.status.success(),
        "迁移 CLI 退出码非零\n{}",
        common::cli_failure_diagnostics(&output)
    );

    let pool = temp.connect_pool().await;
    migrate::verify_applied(&pool)
        .await
        .expect("迁移 CLI 执行后校验应通过");
    pool.close().await;
}

/// 统计当前测试库内仍持有的 advisory 锁（迁移锁泄漏会让计数 > 0）。
async fn advisory_lock_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks l \
         JOIN pg_database d ON d.oid = l.database \
         WHERE l.locktype = 'advisory' AND d.datname = current_database()",
    )
    .fetch_one(pool)
    .await
    .expect("统计 advisory 锁失败")
}

/// 标准 CRC-32（ISO-HDLC，反射多项式 0xEDB88320），与 `sqlx-postgres` 0.9.0 使用的 `crc` 一致。
fn crc32_iso_hdlc(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// SQLx 迁移锁 key：`0x3d32ad9e * CRC32_ISO_HDLC(current_database())`（sqlx-postgres 0.9.0）。
fn migration_lock_id(database: &str) -> i64 {
    0x3d32_ad9e_i64 * i64::from(crc32_iso_hdlc(database.as_bytes()))
}

/// 带独立短 `lock_timeout` 的池，用于让迁移锁争用快速失败（不继承服务/维护默认值）。
async fn bounded_lock_pool(url: &str, lock_ms: u64) -> sqlx::PgPool {
    let value = format!("{lock_ms}ms");
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |connection, _metadata| {
            let value = value.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('lock_timeout', $1, false)")
                    .bind(&value)
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("构建短锁超时池失败")
}

/// 迁移在取得会话迁移锁后失败（脏历史）时，独占连接必须关闭释放锁；
/// 旧实现把带锁的连接还回池，会让后续迁移永久阻塞。用 `pg_locks` 直接回归。
#[tokio::test]
async fn migrate_failure_does_not_strand_session_lock() {
    let (_temp, pool) = TempDatabase::create_migrated().await;
    // 取得锁后失败：dirty_version 命中已标记未完成的基线，SQLx 不会走到 unlock。
    sqlx::query("UPDATE _sqlx_migrations SET success = false WHERE version = 1")
        .execute(&pool)
        .await
        .expect("标记迁移失败状态失败");

    let error = migrate::run(&pool).await.expect_err("脏迁移历史必须失败");
    assert!(
        matches!(error, MigrationError::Execute(_)),
        "期望 SQLx 执行错误，实际 {error}"
    );
    assert_eq!(
        advisory_lock_count(&pool).await,
        0,
        "失败迁移不得把会话迁移锁滞留在池连接上"
    );
    pool.close().await;
}

/// 迁移锁争用：持有迁移锁时另一连接应在有界时间内失败且不建历史表；
/// 释放后另一连接可立即取得迁移锁并完成迁移。
#[tokio::test]
async fn migrate_lock_contention_is_bounded_then_reusable() {
    let temp = TempDatabase::create().await;
    let holder_pool = temp.connect_pool().await;
    let mut holder = holder_pool.acquire().await.expect("取持锁连接失败");
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut *holder)
        .await
        .expect("查询当前库名失败");
    let lock_id = migration_lock_id(&database);
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(lock_id)
        .execute(&mut *holder)
        .await
        .expect("持有迁移锁失败");

    let contender = bounded_lock_pool(temp.url(), 200).await;
    let started = std::time::Instant::now();
    migrate::run(&contender)
        .await
        .expect_err("迁移锁被占用时应失败");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "锁等待必须有明确上限，实际 {:?}",
        started.elapsed()
    );
    // 未取得迁移锁前不得建立历史表或业务表。
    let history: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations')::text")
            .fetch_one(&contender)
            .await
            .expect("查询历史表失败");
    assert!(history.is_none(), "未取得迁移锁不得建立 _sqlx_migrations");

    // 释放后另一连接可立即取得迁移锁并完成迁移（证明争用未留残留）。
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock_id)
        .execute(&mut *holder)
        .await
        .expect("释放迁移锁失败");
    drop(holder);
    migrate::run(&contender)
        .await
        .expect("释放迁移锁后迁移应成功");
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&contender)
            .await
            .expect("查询迁移记录失败");
    // 完整迁移应登记内嵌迁移集合的全部版本；0005 之后即为 [1, 2, 3, 4, 5]。
    let expected: Vec<i64> = MIGRATOR.iter().map(|migration| migration.version).collect();
    assert_eq!(applied, expected, "释放迁移锁后应登记内嵌迁移的全部版本");
    assert_eq!(
        applied,
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        "0011 之后完整迁移版本应为 [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]"
    );

    contender.close().await;
    holder_pool.close().await;
}
