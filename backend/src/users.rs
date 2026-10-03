//! 用户数据访问。
//!
//! 固定查询全部使用 SQLx 编译期宏（`query_file_as!` / `query_file_scalar!` /
//! `query_file!`）与 `queries/*.sql`，由 `.sqlx` 离线元数据提供编译期类型与列检查。
//! 所有业务参数一律 `bind`；真正动态的库名（临时测试库）另用运行期 `AssertSqlSafe`。
//!
//! 并发要点：
//! - 改密/重置/删除/恢复都以 `row_version`（可选再带旧密码、`session_version`）
//!   作为条件更新，避免旧请求覆盖并发重置。
//! - 注册在 `pg_advisory_xact_lock` 下重新检查用户名与规范化邮箱（历史数据允许重复，
//!   不能加唯一索引，也不能自动清理账号），再插入；密码哈希在持锁前完成。

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// PostgreSQL `char_length` limit shared by accounts and attribution snapshots.
/// Migration 0012 widens legacy attribution columns to match existing accounts.
pub const USERNAME_MAX: usize = 255;

/// 注册唯一性检查共用的事务级 advisory lock key。
///
/// 任何 Rust 注册入口都必须先取得该锁；它是唯一约束缺失期间的临时兜底。
/// 值本身无业务含义，只要全进程一致即可。
pub const REGISTER_ADVISORY_LOCK_KEY: i64 = 0x4C59_434F_5249_5301;

/// `users` 行，与 `schema-baseline.sql` 结构一一对应。
///
/// 含 `password`，因此 Debug 经过脱敏，避免后续诊断整份输出历史哈希或明文。
#[derive(Clone, PartialEq, Eq)]
pub struct UserRow {
    pub id: i32,
    pub public_id: Uuid,
    pub username: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    pub avatar_url: Option<String>,
    pub pronouns: Option<String>,
    pub signature: Option<String>,
    pub role: String,
    pub deleted: bool,
    pub deleted_at: Option<DateTime<Utc>>,
    pub session_version: i64,
    pub row_version: i64,
}

impl std::fmt::Debug for UserRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserRow")
            .field("id", &self.id)
            .field("public_id", &self.public_id)
            .field("username", &self.username)
            .field("role", &self.role)
            .field("deleted", &self.deleted)
            .field("session_version", &self.session_version)
            .field("row_version", &self.row_version)
            .field("password", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl UserRow {
    pub fn is_admin(&self) -> bool {
        self.role.eq_ignore_ascii_case("ADMIN")
    }

    pub fn username_or_empty(&self) -> &str {
        self.username.as_deref().unwrap_or("")
    }
}

/// 条件更新返回的版本对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRow {
    pub session_version: i64,
    pub row_version: i64,
}

/// A username may contain `@`. Match both namespaces without changing username
/// case, and count an account only once when its username and email both match.
pub async fn find_active_by_identity(
    pool: &sqlx::PgPool,
    identity: &str,
) -> Result<Vec<UserRow>, sqlx::Error> {
    let email = identity.to_lowercase();
    let (mut candidates, emails) = tokio::try_join!(
        find_active_by_username(pool, identity),
        find_active_by_email(pool, &email),
    )?;
    candidates.extend(emails);
    candidates.sort_unstable_by_key(|user| user.id);
    candidates.dedup_by_key(|user| user.id);
    Ok(candidates)
}

pub async fn find_active_by_id<'e, E>(executor: E, id: i32) -> Result<Option<UserRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(UserRow, "queries/find_active_by_id.sql", id)
        .fetch_optional(executor)
        .await
}

pub async fn find_any_by_id<'e, E>(executor: E, id: i32) -> Result<Option<UserRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(UserRow, "queries/find_any_by_id.sql", id)
        .fetch_optional(executor)
        .await
}

/// 按用户名精确匹配的未删除账号；历史库可能重复，调用方需自行判断数量。
pub async fn find_active_by_username<'e, E>(
    executor: E,
    username: &str,
) -> Result<Vec<UserRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(UserRow, "queries/find_active_by_username.sql", username)
        .fetch_all(executor)
        .await
}

/// 按规范化（小写）邮箱匹配的未删除账号；历史库可能重复。
pub async fn find_active_by_email<'e, E>(
    executor: E,
    email_lower: &str,
) -> Result<Vec<UserRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(UserRow, "queries/find_active_by_email.sql", email_lower)
        .fetch_all(executor)
        .await
}

/// 用户名是否已被任意账号（含软删除）占用。
pub async fn username_exists_any<'e, E>(executor: E, username: &str) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let exists = sqlx::query_file_scalar!("queries/username_exists_any.sql", username)
        .fetch_one(executor)
        .await?;
    Ok(exists.unwrap_or(false))
}

/// 规范化（小写）邮箱是否已被任意账号（含软删除）占用。
pub async fn email_exists_any<'e, E>(executor: E, email_lower: &str) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let exists = sqlx::query_file_scalar!("queries/email_exists_any.sql", email_lower)
        .fetch_one(executor)
        .await?;
    Ok(exists.unwrap_or(false))
}

/// 插入新用户：`role=USER`、`deleted=false`、版本从 0 开始。
pub async fn insert_user<'e, E>(
    executor: E,
    username: &str,
    nickname: &str,
    email_lower: &str,
    password_hash: &str,
) -> Result<UserRow, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(
        UserRow,
        "queries/insert_user.sql",
        Uuid::new_v4(),
        username,
        nickname,
        email_lower,
        password_hash
    )
    .fetch_one(executor)
    .await
}

/// 条件更新资料；返回更新后的行；版本冲突返回 `None`。
pub async fn update_profile<'e, E>(
    executor: E,
    id: i32,
    expected_row_version: i64,
    nickname: Option<&str>,
    pronouns: Option<&str>,
    signature: Option<&str>,
) -> Result<Option<UserRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(
        UserRow,
        "queries/update_profile.sql",
        nickname,
        pronouns,
        signature,
        id,
        expected_row_version
    )
    .fetch_optional(executor)
    .await
}

/// 历史明文成功登录后升级为 BCrypt：条件更新保证并发重置不会被旧登录覆盖。
/// 返回是否命中原密码与版本。
pub async fn upgrade_legacy_password<'e, E>(
    executor: E,
    id: i32,
    expected_password: &str,
    expected_row_version: i64,
    new_password_hash: &str,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query_file!(
        "queries/upgrade_legacy_password.sql",
        new_password_hash,
        id,
        expected_password,
        expected_row_version
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// 改密：要求旧密码与 `row_version`/`session_version` 同时匹配，防止并发重置被旧登录覆盖。
pub async fn change_password<'e, E>(
    executor: E,
    id: i32,
    expected_password: &str,
    expected_row_version: i64,
    expected_session_version: i64,
    new_password_hash: &str,
) -> Result<Option<VersionRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(
        VersionRow,
        "queries/change_password.sql",
        new_password_hash,
        id,
        expected_password,
        expected_row_version,
        expected_session_version
    )
    .fetch_optional(executor)
    .await
}

/// 管理员重置密码；已删除账号不更新。
pub async fn reset_password<'e, E>(
    executor: E,
    id: i32,
    expected_row_version: i64,
    new_password_hash: &str,
) -> Result<Option<VersionRow>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_file_as!(
        VersionRow,
        "queries/reset_password.sql",
        new_password_hash,
        id,
        expected_row_version
    )
    .fetch_optional(executor)
    .await
}

/// 软删除；仅未删除账号可删。返回是否命中。
pub async fn soft_delete<'e, E>(
    executor: E,
    id: i32,
    expected_row_version: i64,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query_file!("queries/soft_delete.sql", id, expected_row_version)
        .execute(executor)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// 恢复；仅已删除账号可恢复。返回是否命中。
pub async fn restore<'e, E>(
    executor: E,
    id: i32,
    expected_row_version: i64,
) -> Result<bool, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let result = sqlx::query_file!("queries/restore.sql", id, expected_row_version)
        .execute(executor)
        .await?;
    Ok(result.rows_affected() == 1)
}

/// 管理员分页查询：`q` 空串表示全部，否则大小写不敏感模糊匹配用户名/昵称/邮箱。
/// 排序固定 `id DESC`（与 Java 一致）。返回 `(总数, 当页行)`。
pub async fn search_for_admin(
    pool: &sqlx::PgPool,
    q: &str,
    page: i64,
    size: i64,
) -> Result<(i64, Vec<UserRow>), sqlx::Error> {
    let total: i64 = sqlx::query_file_scalar!("queries/admin_users_count.sql", q)
        .fetch_one(pool)
        .await?
        .unwrap_or(0);
    let offset = page.saturating_mul(size);
    let rows = sqlx::query_file_as!(UserRow, "queries/admin_users_page.sql", q, size, offset)
        .fetch_all(pool)
        .await?;
    Ok((total, rows))
}
