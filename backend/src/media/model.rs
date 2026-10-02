//! 媒体业务的行与响应 DTO。
//!
//! 这里只定义**媒体业务**需要的数据形状：清单项、清理结果、提案行与若干访问判定
//! 所需的裁剪行。点位响应本身继续复用 `modules::markers::model::MarkerRow`，避免
//! 复制点位字段定义或另立一套身份/可见性系统。

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::modules::markers::model::MarkerRow;

/// 管理员待审图片提案清单项，字段与 Java `/api/admin/markers/pending-images` 一致
/// （8 项，camelCase，`createdAt` 降序）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PendingImageItem {
    pub id: i64,
    pub marker_id: i64,
    pub marker_title: String,
    pub proposer_username: String,
    pub proposer_public_id: Option<String>,
    pub image_url: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// `cleanup-missing-images` 的结果，字段与 Java 对齐。
#[derive(Debug, Clone, Serialize)]
pub struct CleanupResult {
    pub checked: i64,
    pub cleared: i64,
    pub message: String,
}

/// `marker_image_proposals` 的一行（审批事务内以 `FOR UPDATE` 读取）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ImageProposalRow {
    pub id: i64,
    pub marker_id: i64,
    pub marker_title: String,
    pub proposer_username: String,
    pub proposer_public_id: Option<String>,
    pub image_url: String,
    pub status: String,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 清理扫描所需的点位图片引用快照（条件更新以 `id/version/mark_image` 三者匹配）。
///
/// `mark_image` 保留可空类型以匹配 SQLx 对表达式的可空推断；SQL 已过滤非空且带
/// `/uploads/markers/` 前缀，服务层再做一次防御性判断。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MarkerImageRef {
    pub id: i64,
    pub version: i64,
    pub mark_image: Option<String>,
}

/// 用户在头像更新前的状态，用于区分“已删除/不存在”和“行版本不匹配”。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserAvatarStatus {
    pub deleted: bool,
    pub row_version: i64,
}

/// 条件头像更新成功后返回的两列。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdatedAvatarRow {
    pub avatar_url: Option<String>,
    pub row_version: i64,
}

/// 历史图片提案与仍存在的关联点位的连接行。
///
/// `INNER JOIN` 语义由 SQL 保证：点位被删除后不再出现该行，因此历史提案不能给任何
/// 人（包括管理员）授权。`into_marker` 还原为 [`MarkerRow`] 以便复用
/// `markers::model::can_view`。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProposalMarkerRow {
    pub proposer_public_id: Option<String>,
    pub id: i64,
    pub version: i64,
    pub lat: f64,
    pub lng: f64,
    pub category: String,
    pub title: String,
    pub description: Option<String>,
    pub source_language: String,
    pub is_public: bool,
    pub username: String,
    pub user_public_id: Option<String>,
    pub client_request_id: Option<String>,
    pub is_active: bool,
    pub open_time_start: Option<String>,
    pub open_time_end: Option<String>,
    pub review_status: String,
    pub last_edited_by: Option<String>,
    pub last_edited_by_public_id: Option<String>,
    pub last_edited_by_owner: bool,
    pub mark_image: Option<String>,
    pub venue_type: Option<String>,
    pub categories: Vec<String>,
    pub opening_hours_note: Option<String>,
    pub deactivated: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProposalMarkerRow {
    /// 拆分出提案作者公共 ID 与可交给 `can_view` 的点位行。
    pub fn into_parts(self) -> (Option<String>, MarkerRow) {
        let marker = MarkerRow {
            id: self.id,
            version: self.version,
            lat: self.lat,
            lng: self.lng,
            category: self.category,
            title: self.title,
            description: self.description,
            source_language: self.source_language,
            is_public: self.is_public,
            username: self.username,
            user_public_id: self.user_public_id,
            client_request_id: self.client_request_id,
            is_active: self.is_active,
            open_time_start: self.open_time_start,
            open_time_end: self.open_time_end,
            review_status: self.review_status,
            last_edited_by: self.last_edited_by,
            last_edited_by_public_id: self.last_edited_by_public_id,
            last_edited_by_owner: self.last_edited_by_owner,
            mark_image: self.mark_image,
            venue_type: self.venue_type,
            categories: self.categories,
            opening_hours_note: self.opening_hours_note,
            deactivated: self.deactivated,
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        (self.proposer_public_id, marker)
    }
}
