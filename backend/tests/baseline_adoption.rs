//! 已有库基线接管与只读预检的真实 PG 集成测试。
//!
//! 每个用例创建并清理自己的 UUID 临时库；只连接回环合成测试服务，不触碰其他测试库。
//! 结构期望来自已审查基线（测试通过只含 0001 的 fixture 建立 legacy 形状核对），
//! 检查目标库时只读系统目录，不在目标库执行基线 DDL。

mod common;

use std::time::Duration;

use common::{TempDatabase, cli_failure_diagnostics, test_redis_url};
use lycoris_backend::baseline::expected::{BASELINE_TABLES, BASELINE_VERSION};
use lycoris_backend::baseline::{
    self, BaselineDiff, DEFAULT_LOCK_TIMEOUT, DataCounts, HistoryReport, adopt_baseline_with,
    check_baseline,
};
use lycoris_backend::migrate::{self, MIGRATOR, MigrationError};
use sqlx::Acquire as _;
use sqlx::PgPool;
use sqlx::SqlSafeStr as _;
use sqlx::migrate::{Migrate as _, Migration, MigrationType, Migrator};

#[derive(Debug, PartialEq)]
struct Snapshot {
    users: Vec<UserRow>,
    markers: Vec<MarkerRow>,
    translations: Vec<TranslationRow>,
    favorites: Vec<FavoriteRow>,
    sequences: Vec<SequenceRow>,
}

type UserRow = (i32, Option<String>, Option<String>, String, String, bool);
type MarkerRow = (i64, String, f64, f64, i64, String);
type TranslationRow = (i64, i64, String, String, String);
type FavoriteRow = (i64, i64, String);
type SequenceRow = (String, Option<i64>);

async fn snapshot(pool: &PgPool) -> Snapshot {
    let users = sqlx::query_as(
        "SELECT id, username, email, public_id::text, role, deleted FROM users ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .expect("读取 users 失败");
    let markers = sqlx::query_as(
        "SELECT id, title, lat, lng, version, username FROM map_markers ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .expect("读取 map_markers 失败");
    let translations = sqlx::query_as(
        "SELECT id, marker_id, language, title, origin \
         FROM map_marker_translations ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .expect("读取 map_marker_translations 失败");
    let favorites =
        sqlx::query_as("SELECT id, marker_id, user_public_id FROM marker_favorites ORDER BY id")
            .fetch_all(pool)
            .await
            .expect("读取 marker_favorites 失败");
    let sequence_names: Vec<String> = vec![
        "map_markers_id_seq".to_string(),
        "map_marker_translations_id_seq".to_string(),
        "marker_edit_proposals_id_seq".to_string(),
        "marker_favorites_id_seq".to_string(),
        "marker_image_proposals_id_seq".to_string(),
        "users_id_seq".to_string(),
    ];
    let sequences = sqlx::query_as(
        "SELECT sequencename, last_value FROM pg_sequences \
         WHERE schemaname = 'public' AND sequencename = ANY($1) ORDER BY sequencename",
    )
    .bind(&sequence_names)
    .fetch_all(pool)
    .await
    .expect("读取序列失败");
    Snapshot {
        users,
        markers,
        translations,
        favorites,
        sequences,
    }
}

/// 合成一个“Java 形状”的非空库：只有 0001 结构 + 业务行 + 已推进的 identity 序列 + 无 SQLx 历史。
///
/// 明确使用 `create_only_0001`，不把含 0002 生成列的 `create_migrated` 误当 legacy。
async fn java_shaped() -> (TempDatabase, PgPool) {
    let (temp, pool) = TempDatabase::create_only_0001().await;
    seed_java_rows(&pool).await;
    (temp, pool)
}

async fn seed_java_rows(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO users (id, public_id, role, username, email, deleted) VALUES \
         (1, gen_random_uuid(), 'USER', 'alice', 'alice@example.com', false), \
         (2, gen_random_uuid(), 'ADMIN', 'bob', 'bob@example.com', false)",
    )
    .execute(pool)
    .await
    .expect("插入合成用户失败");
    sqlx::query(
        "INSERT INTO map_markers \
         (id, category, created_at, is_active, is_public, last_edited_by_owner, lat, lng, \
          review_status, title, updated_at, username, version, source_language) VALUES \
         (10, 'self_definition', now(), true, true, false, 31.2, 121.4, 'APPROVED', \
          '旧点位', now(), 'alice', 0, 'zh')",
    )
    .execute(pool)
    .await
    .expect("插入合成点位失败");
    sqlx::query(
        "INSERT INTO map_marker_translations \
         (id, marker_id, language, title, source_hash, origin, updated_at) VALUES \
         (5, 10, 'en', 'Old marker', 'hash-1', 'MANUAL', now())",
    )
    .execute(pool)
    .await
    .expect("插入合成译文失败");
    sqlx::query(
        "INSERT INTO marker_favorites (id, created_at, marker_id, user_public_id) VALUES \
         (3, now(), 10, '00000000-0000-0000-0000-000000000001')",
    )
    .execute(pool)
    .await
    .expect("插入合成收藏失败");
    for (sequence, value) in [
        ("users_id_seq", 2_i64),
        ("map_markers_id_seq", 10),
        ("map_marker_translations_id_seq", 5),
        ("marker_favorites_id_seq", 3),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT setval('{sequence}', {value})"
        )))
        .execute(pool)
        .await
        .expect("推进合成序列失败");
    }
}

async fn applied_rows(pool: &PgPool) -> Vec<(i64, bool, Vec<u8>)> {
    sqlx::query_as("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .expect("读取迁移历史失败")
}

async fn history_exists(pool: &PgPool) -> bool {
    let present: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations')::text")
            .fetch_one(pool)
            .await
            .expect("查询历史表失败");
    present.is_some()
}

fn expect_mismatch(error: MigrationError) -> Vec<BaselineDiff> {
    match error {
        MigrationError::BaselineMismatch(mismatch) => mismatch.diffs,
        other => panic!("期望结构不匹配，实际为 {other}"),
    }
}

#[tokio::test]
async fn adopt_preserves_java_rows_and_sequences_and_new_ids_work() {
    let (temp, pool) = java_shaped().await;
    let before = snapshot(&pool).await;

    let report = baseline::adopt_baseline(&pool)
        .await
        .expect("Java 形状库接管应成功");
    assert!(report.schema_ok, "结构应一致: {:?}", report.diffs);
    assert_eq!(
        report.history,
        HistoryReport::Consistent {
            applied: vec![BASELINE_VERSION]
        }
    );

    let rows = applied_rows(&pool).await;
    assert_eq!(rows.len(), 1, "接管后应只有一条迁移历史");
    assert_eq!(rows[0].0, BASELINE_VERSION);
    assert!(rows[0].1, "接管登记必须成功");
    let real_checksum = MIGRATOR
        .iter()
        .next()
        .expect("内嵌基线迁移")
        .checksum
        .to_vec();
    assert_eq!(rows[0].2, real_checksum, "必须登记原始基线真实校验和");

    assert_eq!(before, snapshot(&pool).await, "接管不得改动业务行或序列值");

    // 接管后仍是 0001-only legacy：重复接管幂等，且普通启动校验明确报告缺少 0002。
    let second = baseline::adopt_baseline(&pool)
        .await
        .expect("重复接管应幂等成功");
    assert_eq!(
        second.history,
        HistoryReport::Consistent {
            applied: vec![BASELINE_VERSION]
        }
    );
    assert_eq!(applied_rows(&pool).await.len(), 1, "重复接管不得新增历史");
    assert!(
        matches!(
            migrate::verify_applied(&pool).await.unwrap_err(),
            MigrationError::Missing(2)
        ),
        "只接管 0001 时启动校验应报告缺少 0002"
    );

    // Java 形状写入在接管后、0002 之前仍可用，ID 延续 identity 序列。
    let new_user: i32 = sqlx::query_scalar(
        "INSERT INTO users (public_id, role, username) VALUES (gen_random_uuid(), 'USER', 'carol') \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("接管后新用户插入应成功");
    assert!(new_user > 2, "新用户 ID 必须延续序列，实际 {new_user}");
    let new_marker: i64 = sqlx::query_scalar(
        "INSERT INTO map_markers \
         (category, created_at, is_active, is_public, last_edited_by_owner, lat, lng, \
          review_status, title, updated_at, username, version) VALUES \
         ('self_definition', now(), true, true, false, 1.0, 2.0, 'APPROVED', '新点位', now(), 'carol', 0) \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("接管后新点位插入应成功");
    assert!(new_marker > 10, "新点位 ID 必须延续序列，实际 {new_marker}");

    // 真实流程：接管只登记 0001，随后 `--migrate` 才应用 0002，启动校验再通过。
    let before_migrate = snapshot(&pool).await;
    migrate::run(&pool)
        .await
        .expect("接管后 --migrate 应应用 0002");
    migrate::verify_applied(&pool)
        .await
        .expect("升级后启动只读校验应通过");
    assert_eq!(
        before_migrate,
        snapshot(&pool).await,
        "0002 迁移不得改动业务行或序列值"
    );
    let null_locations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM map_markers WHERE location IS NULL")
            .fetch_one(&pool)
            .await
            .expect("读取生成列失败");
    assert_eq!(null_locations, 0, "现有合法坐标行应生成 location");
    let version_two: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = 2)")
            .fetch_one(&pool)
            .await
            .expect("读取迁移历史失败");
    assert!(version_two, "0002 应登记在迁移历史");

    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_rejects_empty_database_without_creating_history() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
    assert!(
        diffs
            .iter()
            .any(|diff| matches!(diff, BaselineDiff::MissingTable(name) if name == "map_markers")),
        "空库应报告缺少业务表，实际 {diffs:?}"
    );
    assert!(!history_exists(&pool).await, "拒绝后不得创建历史表");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_rejects_structural_mismatches() {
    let cases: &[&str] = &[
        "ALTER TABLE map_markers DROP COLUMN mark_image",
        "ALTER TABLE map_markers ALTER COLUMN title TYPE varchar(150)",
        "ALTER TABLE users ALTER COLUMN deleted SET DEFAULT true",
        "ALTER TABLE marker_favorites DROP CONSTRAINT uq_marker_fav_user_marker",
        "ALTER TABLE map_marker_translations DROP CONSTRAINT map_marker_translations_marker_id_fkey",
        "ALTER TABLE map_markers ALTER COLUMN id DROP IDENTITY",
        "ALTER TABLE map_markers ADD COLUMN ops_note text",
        "DROP TABLE marker_favorites",
    ];
    for mutation in cases {
        let (temp, pool) = TempDatabase::create_only_0001().await;
        sqlx::query(sqlx::AssertSqlSafe((*mutation).to_string()))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("执行结构变更 `{mutation}` 失败: {error}"));

        let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
        assert!(!diffs.is_empty(), "`{mutation}` 应被拒绝");
        assert!(
            !history_exists(&pool).await,
            "`{mutation}` 拒绝后不得创建历史表"
        );
        pool.close().await;
        drop(temp);
    }
}

#[tokio::test]
async fn adopt_rejects_non_ordinary_relation_kind() {
    let (temp, pool) = java_shaped().await;
    sqlx::query("DROP TABLE marker_favorites")
        .execute(&pool)
        .await
        .expect("删除表失败");
    sqlx::query(
        "CREATE VIEW marker_favorites AS \
         SELECT 1::bigint AS id, now() AS created_at, 1::bigint AS marker_id, \
                'x'::varchar AS user_public_id",
    )
    .execute(&pool)
    .await
    .expect("创建同名视图失败");

    let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
    assert!(
        diffs.iter().any(|diff| matches!(
            diff,
            BaselineDiff::UnexpectedRelationKind { table, .. } if table == "marker_favorites"
        )),
        "同名视图必须被拒绝，实际 {diffs:?}"
    );
    assert!(!history_exists(&pool).await);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_rejects_generated_columns() {
    let (temp, pool) = TempDatabase::create_only_0001().await;
    sqlx::query("ALTER TABLE users DROP COLUMN nickname")
        .execute(&pool)
        .await
        .expect("删除 nickname 失败");
    sqlx::query(
        "ALTER TABLE users ADD COLUMN nickname varchar(255) \
         GENERATED ALWAYS AS (lower(username)) STORED",
    )
    .execute(&pool)
    .await
    .expect("新增生成列失败");

    let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
    assert!(
        diffs.iter().any(|diff| matches!(
            diff,
            BaselineDiff::ColumnGenerated { table, column, .. }
                if table == "users" && column == "nickname"
        )),
        "生成列必须被拒绝，实际 {diffs:?}"
    );
    assert!(!history_exists(&pool).await);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_rejects_deferrable_or_not_validated_constraints() {
    let (temp, pool) = TempDatabase::create_only_0001().await;
    sqlx::query(
        "ALTER TABLE map_marker_translations \
         ALTER CONSTRAINT map_marker_translations_marker_id_fkey DEFERRABLE INITIALLY DEFERRED",
    )
    .execute(&pool)
    .await
    .expect("改为可延迟外键失败");
    let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
    assert!(
        diffs.iter().any(|diff| matches!(
            diff,
            BaselineDiff::ConstraintDeferrable { name, .. }
                if name == "map_marker_translations_marker_id_fkey"
        )),
        "可延迟外键必须被拒绝，实际 {diffs:?}"
    );
    pool.close().await;
    drop(temp);

    let (temp, pool) = TempDatabase::create_only_0001().await;
    sqlx::query(
        "ALTER TABLE map_marker_translations \
         DROP CONSTRAINT map_marker_translations_marker_id_fkey",
    )
    .execute(&pool)
    .await
    .expect("删除外键失败");
    sqlx::query(
        "ALTER TABLE map_marker_translations \
         ADD CONSTRAINT map_marker_translations_marker_id_fkey \
         FOREIGN KEY (marker_id) REFERENCES map_markers(id) ON DELETE CASCADE NOT VALID",
    )
    .execute(&pool)
    .await
    .expect("新增未验证外键失败");
    let diffs = expect_mismatch(baseline::adopt_baseline(&pool).await.unwrap_err());
    assert!(
        diffs.iter().any(|diff| matches!(
            diff,
            BaselineDiff::ConstraintNotValidated { name, .. }
                if name == "map_marker_translations_marker_id_fkey"
        )),
        "未验证外键必须被拒绝，实际 {diffs:?}"
    );
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_fails_controlled_when_business_table_is_exclusively_locked() {
    let (temp, pool) = java_shaped().await;
    let mut locker = pool.acquire().await.expect("获取锁持有连接失败");
    let mut tx = locker.begin().await.expect("开启事务失败");
    sqlx::query("LOCK TABLE public.map_markers IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .expect("独占锁失败");

    let error = adopt_baseline_with(
        &pool,
        &MIGRATOR,
        BASELINE_VERSION,
        Duration::from_millis(300),
    )
    .await
    .expect_err("业务表被独占锁时接管必须受控失败");
    assert!(
        matches!(error, MigrationError::LockUnavailable),
        "实际 {error}"
    );
    assert!(!history_exists(&pool).await, "受控失败不得创建历史表");

    tx.rollback().await.expect("释放业务表锁失败");
    drop(locker);
    baseline::adopt_baseline(&pool)
        .await
        .expect("释放后接管应成功");
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn share_lock_blocks_writes_and_ddl_until_released() {
    let (temp, pool) = TempDatabase::create_migrated().await;
    let mut holder = pool.acquire().await.expect("获取持锁连接失败");
    let mut tx = holder.begin().await.expect("开启事务失败");
    sqlx::query("LOCK TABLE public.map_markers IN SHARE MODE")
        .execute(&mut *tx)
        .await
        .expect("加 SHARE 锁失败");

    let mut other = pool.acquire().await.expect("获取第二连接失败");
    sqlx::query("SET lock_timeout = '300ms'")
        .execute(&mut *other)
        .await
        .expect("设置 lock_timeout 失败");
    let write = sqlx::query(
        "INSERT INTO map_markers \
         (category, created_at, is_active, is_public, last_edited_by_owner, lat, lng, \
          review_status, title, updated_at, username) VALUES \
         ('self_definition', now(), true, true, false, 1, 2, 'APPROVED', 'blocked', now(), 'u')",
    )
    .execute(&mut *other)
    .await
    .expect_err("SHARE 锁应阻断写入");
    assert_eq!(error_code(&write).as_deref(), Some("55P03"));
    let ddl = sqlx::query("ALTER TABLE map_markers ADD COLUMN probe integer")
        .execute(&mut *other)
        .await
        .expect_err("SHARE 锁应阻断 DDL");
    assert_eq!(error_code(&ddl).as_deref(), Some("55P03"));

    tx.rollback().await.expect("释放 SHARE 锁失败");
    drop(holder);
    sqlx::query("RESET lock_timeout")
        .execute(&mut *other)
        .await
        .expect("重置 lock_timeout 失败");
    sqlx::query(
        "INSERT INTO map_markers \
         (category, created_at, is_active, is_public, last_edited_by_owner, lat, lng, \
          review_status, title, updated_at, username) VALUES \
         ('self_definition', now(), true, true, false, 1, 2, 'APPROVED', 'ok', now(), 'u')",
    )
    .execute(&mut *other)
    .await
    .expect("释放后写入应成功");
    drop(other);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn registration_failure_rolls_back_without_half_record() {
    let (temp, pool) = TempDatabase::create_only_0001_migrated().await;
    sqlx::query(
        "CREATE FUNCTION reject_history_insert() RETURNS trigger AS \
         $$ BEGIN RAISE EXCEPTION 'blocked insert'; END $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .expect("创建触发器函数失败");
    sqlx::query(
        "CREATE TRIGGER reject_history BEFORE INSERT ON _sqlx_migrations \
         FOR EACH ROW EXECUTE FUNCTION reject_history_insert()",
    )
    .execute(&pool)
    .await
    .expect("创建触发器失败");
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 1")
        .execute(&pool)
        .await
        .expect("删除基线记录失败");

    let error = baseline::adopt_baseline(&pool).await.unwrap_err();
    assert!(
        matches!(error, MigrationError::Execute(_)),
        "插入被触发器拒绝应报执行错误，实际 {error}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("读取行数失败");
    assert_eq!(count, 0, "登记失败不得留下半条记录");

    sqlx::query("DROP TRIGGER reject_history ON _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("删除触发器失败");
    baseline::adopt_baseline(&pool)
        .await
        .expect("移除故障后接管应成功");
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn cancelled_adoption_releases_lock_and_pool_stays_usable() {
    let (temp, pool) = java_shaped().await;
    // 独占业务表使接管先获得迁移锁、再阻塞在业务表锁上。
    let mut locker = pool.acquire().await.expect("获取锁持有连接失败");
    let mut tx = locker.begin().await.expect("开启事务失败");
    sqlx::query("LOCK TABLE public.map_markers IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .expect("独占锁失败");

    let cancelled = tokio::time::timeout(
        Duration::from_millis(500),
        adopt_baseline_with(&pool, &MIGRATOR, BASELINE_VERSION, Duration::from_secs(30)),
    )
    .await;
    assert!(cancelled.is_err(), "持有业务表锁时接管应被取消");

    tx.rollback().await.expect("释放业务表锁失败");
    drop(locker);

    // 被取消的接管连接必须已随 future drop 关闭并释放迁移锁。
    baseline::adopt_baseline_with(&pool, &MIGRATOR, BASELINE_VERSION, Duration::from_secs(5))
        .await
        .expect("取消后应能重新获取迁移锁并接管");
    assert_eq!(applied_rows(&pool).await.len(), 1);
    let usable: i64 = sqlx::query_scalar("SELECT 1::bigint")
        .fetch_one(&pool)
        .await
        .expect("连接池应保持可用");
    assert_eq!(usable, 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn check_baseline_reports_missing_tables_without_database_error() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    let report = check_baseline(&pool)
        .await
        .expect("缺表时应返回可读报告而非数据库错误");
    assert!(!report.schema_ok);
    assert!(
        report
            .diffs
            .iter()
            .any(|diff| matches!(diff, BaselineDiff::MissingTable(_)))
    );
    assert_eq!(report.counts, DataCounts::default());
    assert!(!report.counts_available, "缺表时计数不得当作已测量");
    assert!(!history_exists(&pool).await, "只读预检不得创建历史表");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn check_baseline_marks_counts_unavailable_on_incompatible_columns() {
    let (temp, pool) = TempDatabase::create_only_0001().await;
    sqlx::query("ALTER TABLE map_markers DROP COLUMN lng")
        .execute(&pool)
        .await
        .expect("删除坐标列失败");
    let report = check_baseline(&pool)
        .await
        .expect("列缺失时应返回可读报告而非 SQL 错误");
    assert!(!report.schema_ok);
    assert!(
        report.diffs.iter().any(|diff| matches!(
            diff,
            BaselineDiff::MissingColumn { table, column }
                if table == "map_markers" && column == "lng"
        )),
        "应报告缺失列，实际 {:?}",
        report.diffs
    );
    assert!(
        !report.counts_available,
        "列不兼容时不得以未执行的 0 冒充计数"
    );
    assert!(!history_exists(&pool).await, "只读预检不得创建历史表");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn new_history_table_rolls_back_when_registration_fails() {
    let (temp, pool) = java_shaped().await;
    assert!(!history_exists(&pool).await, "前置：无历史表");
    let before = snapshot(&pool).await;

    // 自定义迁移集合不含基线版本：ensure_migrations_table 之后、skip 之前必定失败。
    let future = Migration::new(
        2,
        "future".into(),
        MigrationType::Simple,
        "SELECT 1".into_sql_str(),
        false,
    );
    let custom = Migrator::with_migrations(vec![future]);
    let error = adopt_baseline_with(&pool, &custom, BASELINE_VERSION, DEFAULT_LOCK_TIMEOUT)
        .await
        .expect_err("基线不在迁移集合时应失败");
    assert!(
        matches!(error, MigrationError::BaselineNotEmbedded(1)),
        "实际 {error}"
    );
    assert!(
        !history_exists(&pool).await,
        "新建的历史表必须随事务回滚，不得残留"
    );
    assert_eq!(before, snapshot(&pool).await, "业务行与序列不得改变");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_rejects_history_problems_without_rewriting() {
    // 错误校验和
    let (temp, pool) = TempDatabase::create_only_0001_migrated().await;
    sqlx::query(
        "UPDATE _sqlx_migrations SET checksum = decode('deadbeef', 'hex') WHERE version = 1",
    )
    .execute(&pool)
    .await
    .expect("篡改校验和失败");
    let error = baseline::adopt_baseline(&pool).await.unwrap_err();
    assert!(
        matches!(error, MigrationError::ChecksumMismatch(1)),
        "实际 {error}"
    );
    let checksum: Vec<u8> =
        sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations WHERE version = 1")
            .fetch_one(&pool)
            .await
            .expect("读取校验和失败");
    assert_eq!(checksum, decode_hex("deadbeef"), "拒绝后不得改写校验和");
    pool.close().await;
    drop(temp);

    // 未知迁移
    let (temp, pool) = TempDatabase::create_only_0001_migrated().await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations \
         (version, description, installed_on, success, checksum, execution_time) \
         VALUES (999, 'unknown', now(), true, decode('00', 'hex'), 0)",
    )
    .execute(&pool)
    .await
    .expect("写入未知迁移失败");
    let error = baseline::adopt_baseline(&pool).await.unwrap_err();
    assert!(
        matches!(error, MigrationError::UnknownApplied(999)),
        "实际 {error}"
    );
    assert_eq!(applied_rows(&pool).await.len(), 2, "拒绝后不得删除未知记录");
    pool.close().await;
    drop(temp);

    // 未完成迁移
    let (temp, pool) = TempDatabase::create_only_0001_migrated().await;
    sqlx::query("UPDATE _sqlx_migrations SET success = false WHERE version = 1")
        .execute(&pool)
        .await
        .expect("标记失败迁移失败");
    let error = baseline::adopt_baseline(&pool).await.unwrap_err();
    assert!(matches!(error, MigrationError::Failed(1)), "实际 {error}");
    let success: bool =
        sqlx::query_scalar("SELECT success FROM _sqlx_migrations WHERE version = 1")
            .fetch_one(&pool)
            .await
            .expect("读取 success 失败");
    assert!(!success, "拒绝后不得改写失败记录");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_confirms_clean_sqlx_database_idempotently() {
    let (temp, pool) = TempDatabase::create_only_0001_migrated().await;
    for _ in 0..2 {
        let report = baseline::adopt_baseline(&pool)
            .await
            .expect("正确库确认应成功");
        assert_eq!(
            report.history,
            HistoryReport::Consistent {
                applied: vec![BASELINE_VERSION]
            }
        );
    }
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn concurrent_adoptions_leave_single_history_record() {
    let (temp, pool) = java_shaped().await;
    let mut handles = Vec::new();
    for _ in 0..4 {
        let clone = pool.clone();
        handles.push(tokio::spawn(async move {
            baseline::adopt_baseline(&clone).await
        }));
    }
    for handle in handles {
        handle
            .await
            .expect("接管任务不应 panic")
            .expect("并发接管应成功");
    }
    assert_eq!(
        applied_rows(&pool).await.len(),
        1,
        "并发接管只应登记一条历史"
    );
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_lock_timeout_does_not_register_partial_history() {
    let (temp, pool) = java_shaped().await;
    let mut holder = pool.acquire().await.expect("获取连接失败");
    holder.lock().await.expect("占用迁移锁失败");

    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        adopt_baseline_with(
            &pool,
            &MIGRATOR,
            BASELINE_VERSION,
            Duration::from_millis(300),
        ),
    )
    .await
    .expect("锁等待上限未生效，测试不应无限阻塞");
    let error = outcome.expect_err("持锁时接管必须失败");
    assert!(
        matches!(error, MigrationError::LockUnavailable),
        "实际 {error}"
    );
    assert!(!history_exists(&pool).await, "锁超时不得创建历史表");

    holder.unlock().await.expect("释放迁移锁失败");
    drop(holder);
    baseline::adopt_baseline(&pool)
        .await
        .expect("释放锁后接管应成功");
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn check_baseline_is_read_only_and_reports_decision_counts() {
    let (temp, pool) = TempDatabase::create_only_0001().await;
    sqlx::query(
        "INSERT INTO users (id, public_id, role, username, email) VALUES \
         (1, gen_random_uuid(), 'USER', 'alice', 'Alice@Example.com'), \
         (2, gen_random_uuid(), 'USER', 'alice', 'alice@example.com'), \
         (3, gen_random_uuid(), 'USER', 'bob', 'bob@example.com')",
    )
    .execute(&pool)
    .await
    .expect("插入重复用户失败");
    sqlx::query(
        "INSERT INTO map_markers \
         (id, category, created_at, is_active, is_public, last_edited_by_owner, lat, lng, \
          review_status, title, updated_at, username, version) VALUES \
         (100, 'self_definition', now(), true, true, false, 31.0, 121.0, 'APPROVED', 'ok', now(), 'alice', 0), \
         (101, 'self_definition', now(), true, true, false, 'NaN'::float8, 121.0, 'APPROVED', 'nan', now(), 'alice', 0), \
         (102, 'self_definition', now(), true, true, false, 31.0, 'Infinity'::float8, 'APPROVED', 'inf', now(), 'alice', 0), \
         (103, 'self_definition', now(), true, true, false, 91.0, 121.0, 'APPROVED', 'range', now(), 'alice', 0)",
    )
    .execute(&pool)
    .await
    .expect("插入异常坐标失败");

    let report = check_baseline(&pool).await.expect("只读预检应成功");
    assert!(report.schema_ok, "结构应一致: {:?}", report.diffs);
    assert_eq!(report.history, HistoryReport::Absent);
    assert!(report.counts_available, "结构一致时计数应执行");
    assert!(
        report.counts.duplicate_username_groups >= 1,
        "应报告重复有效用户名组"
    );
    assert!(
        report.counts.duplicate_normalized_email_groups >= 1,
        "应报告重复规范化邮箱组"
    );
    assert_eq!(report.counts.nan_coordinate_rows, 1, "应明确识别 NaN");
    assert_eq!(
        report.counts.infinite_coordinate_rows, 1,
        "应明确识别 Infinity"
    );
    assert!(
        report.counts.non_finite_coordinate_rows >= 2,
        "应报告非有限坐标"
    );
    assert!(
        report.counts.invalid_coordinate_rows >= 3,
        "应报告不合法经纬度行"
    );
    assert!(!history_exists(&pool).await, "只读预检不得创建历史表");
    let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM map_markers")
        .fetch_one(&pool)
        .await
        .expect("读取行数失败");
    assert_eq!(markers, 4, "只读预检必须保留所有行");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn migrate_refuses_existing_business_tables_and_points_to_adoption() {
    let (temp, pool) = java_shaped().await;
    let error = migrate::run(&pool).await.unwrap_err();
    assert!(
        matches!(error, MigrationError::ExistingSchemaNeedsAdoption),
        "实际 {error}"
    );
    assert!(!history_exists(&pool).await, "拒绝后不得创建历史表");

    baseline::adopt_baseline(&pool).await.expect("接管应成功");
    migrate::run(&pool)
        .await
        .expect("接管后 --migrate 应无待执行迁移且成功");
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_allows_unrelated_extra_tables() {
    let (temp, pool) = java_shaped().await;
    sqlx::query("CREATE TABLE ops_audit (id bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, note text)")
        .execute(&pool)
        .await
        .expect("创建运维表失败");
    baseline::adopt_baseline(&pool)
        .await
        .expect("额外运维表不应阻止接管");
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn adopt_does_not_mark_future_migrations_as_applied() {
    let (temp, pool) = java_shaped().await;
    let baseline_migration = MIGRATOR.iter().next().expect("内嵌基线迁移").clone();
    let future = Migration::new(
        2,
        "future".into(),
        MigrationType::Simple,
        "SELECT 1".into_sql_str(),
        false,
    );
    let custom = Migrator::with_migrations(vec![baseline_migration, future]);

    adopt_baseline_with(&pool, &custom, BASELINE_VERSION, DEFAULT_LOCK_TIMEOUT)
        .await
        .expect("自定义迁移集合接管应成功");
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("读取版本失败");
    assert_eq!(versions, vec![BASELINE_VERSION], "不得登记未来迁移");
    // 静态 MIGRATOR 现含 0001+0002；自定义集合只登记 0001，启动校验应报告缺少 0002。
    assert!(
        matches!(
            migrate::verify_applied(&pool).await.unwrap_err(),
            MigrationError::Missing(2)
        ),
        "adopt 不得把未执行的后续迁移记为已应用"
    );
    pool.close().await;
    drop(temp);
}

#[tokio::test]
async fn cli_adopt_and_check_baseline_are_wired() {
    let (temp, pool) = java_shaped().await;
    let bin = env!("CARGO_BIN_EXE_lycoris-backend");

    let check = std::process::Command::new(bin)
        .arg("--check-baseline")
        .env("DATABASE_URL", temp.url())
        .env("REDIS_URL", test_redis_url())
        .output()
        .expect("运行 --check-baseline 失败");
    assert!(
        check.status.success(),
        "--check-baseline 应对一致结构退出 0\n{}",
        cli_failure_diagnostics(&check)
    );
    assert!(
        !history_exists(&pool).await,
        "--check-baseline 不得创建历史表"
    );

    let adopt = std::process::Command::new(bin)
        .arg("--adopt-baseline")
        .env("DATABASE_URL", temp.url())
        .env("REDIS_URL", test_redis_url())
        .output()
        .expect("运行 --adopt-baseline 失败");
    assert!(
        adopt.status.success(),
        "--adopt-baseline 应成功\n{}",
        cli_failure_diagnostics(&adopt)
    );
    assert_eq!(applied_rows(&pool).await.len(), 1);
    pool.close().await;
    drop(temp);
}

/// 服务是否真正启动（`tracing` 默认写 stdout，必须同时检查两路输出）。
fn started_service(output: &std::process::Output) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("HTTP 服务已启动") || stderr.contains("HTTP 服务已启动")
}

#[tokio::test]
async fn cli_rejects_unknown_and_conflicting_arguments() {
    let bin = env!("CARGO_BIN_EXE_lycoris-backend");

    let unknown = std::process::Command::new(bin)
        .arg("--bogus")
        .env_remove("DATABASE_URL")
        .env_remove("REDIS_URL")
        .output()
        .expect("运行未知参数失败");
    assert!(
        !unknown.status.success(),
        "未知参数必须失败\n{}",
        cli_failure_diagnostics(&unknown)
    );
    assert!(!started_service(&unknown), "未知参数不得启动服务");

    let conflicting = std::process::Command::new(bin)
        .args(["--migrate", "--adopt-baseline"])
        .env_remove("DATABASE_URL")
        .env_remove("REDIS_URL")
        .output()
        .expect("运行互斥参数失败");
    assert!(
        !conflicting.status.success(),
        "互斥参数必须失败\n{}",
        cli_failure_diagnostics(&conflicting)
    );
    assert!(!started_service(&conflicting), "互斥参数不得启动服务");
}

#[tokio::test]
async fn cli_healthcheck_rejects_extra_or_conflicting_commands() {
    let bin = env!("CARGO_BIN_EXE_lycoris-backend");
    for case in [
        vec!["--healthcheck", "--migrate"],
        vec!["--migrate", "--healthcheck"],
        vec!["--healthcheck", "/health/ready", "--adopt-baseline"],
        vec!["--healthcheck", "--healthcheck"],
        vec!["--healthcheck", "/health/ready", "extra"],
    ] {
        let output = std::process::Command::new(bin)
            .args(&case)
            .env_remove("DATABASE_URL")
            .env_remove("REDIS_URL")
            .output()
            .expect("运行 healthcheck 参数组合失败");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{case:?} 必须因参数错误失败\n{}",
            cli_failure_diagnostics(&output)
        );
        assert!(
            stderr.contains("参数错误"),
            "{case:?} 应在解析阶段失败而非进入探针\n{}",
            cli_failure_diagnostics(&output)
        );
        assert!(
            !stderr.contains("healthcheck:") && !started_service(&output),
            "{case:?} 不得执行探针或启动服务\n{}",
            cli_failure_diagnostics(&output)
        );
    }
}

#[tokio::test]
async fn cli_healthcheck_runs_without_database_config() {
    // 取一个刚释放、确定无人监听的本地端口，避免误连其他服务。
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定回环端口失败");
        listener.local_addr().expect("读取端口失败").port()
    };
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lycoris-backend"))
        .args(["--healthcheck", "/health/ready"])
        .env_remove("DATABASE_URL")
        .env_remove("REDIS_URL")
        .env("SERVER_PORT", port.to_string())
        .output()
        .expect("运行 --healthcheck 失败");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 健康检查不得读取数据库/Redis 配置；失败只因探针连不上而非缺少连接串。
    assert!(
        !stdout.contains("DATABASE_URL") && !stderr.contains("DATABASE_URL"),
        "健康检查不得读取 DATABASE_URL\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(
        !stdout.contains("REDIS_URL") && !stderr.contains("REDIS_URL"),
        "健康检查不得读取 REDIS_URL\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(
        stderr.contains("healthcheck:"),
        "应实际执行探针并报告失败\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(
        !started_service(&output),
        "健康检查不得启动 HTTP 服务\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(
        !output.status.success(),
        "端口无监听时探针必须非零退出\n{}",
        cli_failure_diagnostics(&output)
    );
}

#[tokio::test]
async fn cli_help_prints_usage_without_starting_service() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lycoris-backend"))
        .arg("--help")
        .env_remove("DATABASE_URL")
        .env_remove("REDIS_URL")
        .output()
        .expect("运行 --help 失败");
    assert!(
        output.status.success(),
        "--help 应成功退出\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("用法:"),
        "--help 应打印用法\n{}",
        cli_failure_diagnostics(&output)
    );
    assert!(!started_service(&output), "--help 不得启动服务");
}

#[tokio::test]
async fn cli_check_baseline_fails_on_incompatible_database() {
    let temp = TempDatabase::create().await;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_lycoris-backend"))
        .arg("--check-baseline")
        .env("DATABASE_URL", temp.url())
        .env("REDIS_URL", test_redis_url())
        .output()
        .expect("运行 --check-baseline 失败");
    assert!(
        !output.status.success(),
        "不兼容库的预检应非零退出\n{}",
        cli_failure_diagnostics(&output)
    );
}

#[test]
fn baseline_table_list_matches_snapshot() {
    assert_eq!(BASELINE_TABLES.len(), 6);
    assert_eq!(BASELINE_VERSION, 1);
}

fn decode_hex(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() / 2);
    let bytes = value.as_bytes();
    for pair in bytes.chunks(2) {
        let text = std::str::from_utf8(pair).expect("十六进制文本");
        out.push(u8::from_str_radix(text, 16).expect("十六进制字节"));
    }
    out
}

fn error_code(error: &sqlx::Error) -> Option<String> {
    match error {
        sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
        _ => None,
    }
}

#[tokio::test]
async fn attribution_migration_preserves_existing_long_accounts_and_business_rows() {
    let temp = TempDatabase::create().await;
    let pool = temp.connect_pool().await;
    let previous = Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < 12)
            .cloned()
            .collect(),
    );
    previous.run(&pool).await.unwrap();
    seed_java_rows(&pool).await;
    let username = "旧".repeat(255);
    sqlx::query("UPDATE users SET username=$1 WHERE id=1")
        .bind(&username)
        .execute(&pool)
        .await
        .unwrap();
    let before = snapshot(&pool).await;
    migrate::run(&pool).await.unwrap();
    assert_eq!(
        snapshot(&pool).await,
        before,
        "widening must not rename users or rewrite attribution"
    );
    let lengths: Vec<i32> = sqlx::query_scalar(
        "SELECT character_maximum_length::int FROM information_schema.columns
         WHERE table_schema='public' AND (
           (table_name='map_markers' AND column_name IN ('username','last_edited_by')) OR
           (table_name IN ('marker_edit_proposals','marker_image_proposals')
              AND column_name IN ('proposer_username','reviewed_by')))",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(lengths, vec![255; 6]);
    // A user that already existed before migration can now write its complete name.
    sqlx::query("UPDATE map_markers SET username=$1, last_edited_by=$1 WHERE id=10")
        .bind(&username)
        .execute(&pool)
        .await
        .unwrap();
    let stored: String = sqlx::query_scalar("SELECT username FROM map_markers WHERE id=10")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, username);
    migrate::run(&pool).await.unwrap();
    pool.close().await;
}
