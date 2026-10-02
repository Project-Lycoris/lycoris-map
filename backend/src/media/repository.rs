//! 媒体业务固定 SQL 仓储。
//!
//! 所有语句来自 `src/media/sql/` 下的 `.sql` 文件，通过 `query_file_*!` 宏编译期校验，
//! 参数全部绑定，**没有** `AssertSqlSafe`、字符串拼接或动态 SQL。事务内步骤显式接收
//! `&mut PgConnection`，保证锁与更新在同一连接/事务上执行。

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::media::model::{
    ImageProposalRow, MarkerImageRef, PendingImageItem, ProposalMarkerRow, UpdatedAvatarRow,
    UserAvatarStatus,
};
use crate::modules::markers::model::MarkerRow;

/// 媒体业务仓储。仅持有连接池句柄。
#[derive(Debug, Clone)]
pub struct MediaRepository {
    pool: PgPool,
}

impl MediaRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 连接池句柄，供服务层开启事务。
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 按公共 ID 读取未删除用户的头像 URL（缺失或为空均为 `None`）。
    pub async fn avatar_url_by_public_id(
        &self,
        public_id: Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        let value = sqlx::query_file_scalar!("src/media/sql/avatar_by_public_id.sql", public_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(value.flatten())
    }

    /// 按内部用户 ID 读取未删除用户的头像 URL。
    pub async fn avatar_url_by_user_id(&self, user_id: i32) -> Result<Option<String>, sqlx::Error> {
        let value = sqlx::query_file_scalar!("src/media/sql/avatar_by_user_id.sql", user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(value.flatten())
    }

    /// 读取用户当前删除标记与行版本；不存在返回 `None`。
    pub async fn user_avatar_status(
        &self,
        user_id: i32,
    ) -> Result<Option<UserAvatarStatus>, sqlx::Error> {
        sqlx::query_file_as!(
            UserAvatarStatus,
            "src/media/sql/user_avatar_status.sql",
            user_id
        )
        .fetch_optional(&self.pool)
        .await
    }

    /// 条件更新头像：仅当未删除且行版本匹配时写入并推进 `row_version`。
    pub async fn update_avatar_conditional(
        &self,
        conn: &mut PgConnection,
        user_id: i32,
        expected_row_version: i64,
        avatar_url: &str,
    ) -> Result<Option<UpdatedAvatarRow>, sqlx::Error> {
        sqlx::query_file_as!(
            UpdatedAvatarRow,
            "src/media/sql/update_avatar_conditional.sql",
            avatar_url,
            user_id,
            expected_row_version
        )
        .fetch_optional(&mut *conn)
        .await
    }

    /// 按主键读取点位（不加锁，供提交前的廉价可见性检查）。
    pub async fn marker_by_id(&self, id: i64) -> Result<Option<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(MarkerRow, "src/media/sql/marker_by_id.sql", id)
            .fetch_optional(&self.pool)
            .await
    }

    /// 事务内以 `FOR UPDATE` 锁定点位。
    pub async fn lock_marker_by_id(
        &self,
        conn: &mut PgConnection,
        id: i64,
    ) -> Result<Option<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(MarkerRow, "src/media/sql/lock_marker_by_id.sql", id)
            .fetch_optional(&mut *conn)
            .await
    }

    /// 直接引用该图片 URL 的全部点位（可见性由服务层判定）。
    pub async fn markers_by_mark_image(&self, url: &str) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(MarkerRow, "src/media/sql/markers_by_mark_image.sql", url)
            .fetch_all(&self.pool)
            .await
    }

    /// 历史图片提案与仍存在点位的连接行（点位不存在则不返回）。
    pub async fn proposals_with_marker_by_image_url(
        &self,
        url: &str,
    ) -> Result<Vec<ProposalMarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            ProposalMarkerRow,
            "src/media/sql/proposals_with_marker_by_image_url.sql",
            url
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 插入 PENDING 图片提案，返回提案 ID。
    pub async fn insert_image_proposal(
        &self,
        conn: &mut PgConnection,
        marker_id: i64,
        marker_title: &str,
        proposer_username: &str,
        proposer_public_id: Option<&str>,
        image_url: &str,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query_file_scalar!(
            "src/media/sql/insert_image_proposal.sql",
            marker_id,
            marker_title,
            proposer_username,
            proposer_public_id,
            image_url
        )
        .fetch_one(&mut *conn)
        .await
    }

    /// PENDING 图片提案清单（`created_at DESC`）。
    pub async fn pending_images(&self) -> Result<Vec<PendingImageItem>, sqlx::Error> {
        sqlx::query_file_as!(PendingImageItem, "src/media/sql/pending_images.sql")
            .fetch_all(&self.pool)
            .await
    }

    /// 事务内以 `FOR UPDATE` 锁定提案行。
    pub async fn lock_image_proposal(
        &self,
        conn: &mut PgConnection,
        id: i64,
    ) -> Result<Option<ImageProposalRow>, sqlx::Error> {
        sqlx::query_file_as!(
            ImageProposalRow,
            "src/media/sql/lock_image_proposal.sql",
            id
        )
        .fetch_optional(&mut *conn)
        .await
    }

    /// 更新点位图片并推进版本，返回更新后的行。
    pub async fn append_marker_photo(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        marker_id: i64,
        url: &str,
        order: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query_file!(
            "src/media/sql/insert_marker_photo.sql",
            marker_id,
            url,
            order
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
    pub async fn update_marker_mark_image(
        &self,
        conn: &mut PgConnection,
        image_url: &str,
        marker_id: i64,
    ) -> Result<Option<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/media/sql/update_marker_mark_image.sql",
            image_url,
            marker_id
        )
        .fetch_optional(&mut *conn)
        .await
    }

    /// 一次性将 PENDING 提案置为 APPROVED，返回受影响行数。
    pub async fn approve_image_proposal(
        &self,
        conn: &mut PgConnection,
        reviewer: &str,
        proposal_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query_file!(
            "src/media/sql/approve_image_proposal.sql",
            reviewer,
            proposal_id
        )
        .execute(&mut *conn)
        .await?;
        Ok(result.rows_affected())
    }

    /// 一次性将 PENDING 提案置为 REJECTED，返回受影响行数。
    pub async fn reject_image_proposal(
        &self,
        conn: &mut PgConnection,
        reviewer: &str,
        proposal_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query_file!(
            "src/media/sql/reject_image_proposal.sql",
            reviewer,
            proposal_id
        )
        .execute(&mut *conn)
        .await?;
        Ok(result.rows_affected())
    }

    /// 清理扫描：仍以 `/uploads/markers/` 开头的点位图片引用快照。
    pub async fn marker_images_for_cleanup(&self) -> Result<Vec<MarkerImageRef>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerImageRef,
            "src/media/sql/marker_images_for_cleanup.sql"
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 仅当 `id/version/mark_image` 仍匹配时条件清空，返回受影响行数。
    pub async fn clear_mark_image_conditional(
        &self,
        conn: &mut PgConnection,
        id: i64,
        version: i64,
        mark_image: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query_file!(
            "src/media/sql/clear_mark_image_conditional.sql",
            id,
            version,
            mark_image
        )
        .execute(&mut *conn)
        .await?;
        Ok(result.rows_affected())
    }
}
