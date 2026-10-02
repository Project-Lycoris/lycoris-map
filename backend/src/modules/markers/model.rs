//! 数据行与响应 DTO。
//!
//! [`MarkerRow`] 直接映射 `map_markers` 的数据库列，供仓储层与访问判定使用；
//! [`MarkerDto`] 是 HTTP 响应形状，字段与 Java `MapMarker` 一致（camelCase）。
//! 两者刻意分离：读取时对副本计算类别归一与 `isActive`，绝不回写数据库或推进
//! `version`。

use chrono::{DateTime, Utc};
use serde::Serialize;

/// `map_markers` 的一行。字段名与列名一一对应，交给 `sqlx::FromRow` 按名映射。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MarkerRow {
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

/// `map_marker_translations` 中与读取相关的列。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TranslationRow {
    pub marker_id: i64,
    pub language: String,
    pub title: String,
    pub description: Option<String>,
    pub source_hash: String,
}

/// 响应 DTO，字段与 Java `MapMarker` 的成功响应一致。
///
/// `contentLanguage` 为读取时计算的临时字段（原实体为 `@Transient`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkerDto {
    pub photos: Vec<MarkerPhoto>,
    pub id: i64,
    pub version: i64,
    pub lat: f64,
    pub lng: f64,
    pub category: String,
    pub title: String,
    pub description: Option<String>,
    pub source_language: String,
    pub content_language: String,
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
    /// Venue tags are valid whenever categories include an accessible toilet.
    pub venue_type: Option<String>,
    pub categories: Vec<String>,
    pub opening_hours_note: Option<String>,
    /// 服务端可用时区（IANA 名称），与 `availability_zone` 配置一致；只读派生字段。
    pub hours_timezone: String,
    pub deactivated: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MarkerDto {
    /// 由数据库行与读取期计算结果构造响应 DTO。
    pub fn from_row(
        row: MarkerRow,
        category: String,
        title: String,
        description: Option<String>,
        content_language: &str,
        is_active: bool,
        hours_timezone: &str,
    ) -> Self {
        Self {
            photos: Vec::new(),
            id: row.id,
            version: row.version,
            lat: row.lat,
            lng: row.lng,
            category,
            title,
            description,
            source_language: row.source_language,
            content_language: content_language.to_string(),
            is_public: row.is_public,
            username: row.username,
            user_public_id: row.user_public_id,
            client_request_id: row.client_request_id,
            is_active,
            open_time_start: row.open_time_start,
            open_time_end: row.open_time_end,
            review_status: row.review_status,
            last_edited_by: row.last_edited_by,
            last_edited_by_public_id: row.last_edited_by_public_id,
            last_edited_by_owner: row.last_edited_by_owner,
            mark_image: row.mark_image,
            venue_type: row.venue_type,
            categories: row
                .categories
                .iter()
                .map(|value| super::localization::normalize_category_read(value))
                .collect(),
            opening_hours_note: row.opening_hours_note,
            hours_timezone: hours_timezone.to_string(),
            deactivated: row.deactivated,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Identity used for resource-level visibility checks.
/// Construct this from authenticated server state, never from request fields.
/// Anonymous reads pass `None`; only administrators may view deactivated records.
#[derive(Debug, Clone, Copy)]
pub struct Viewer<'a> {
    pub public_id: Option<&'a str>,
    pub role: &'a str,
    pub deleted: bool,
}

/// `isPublic(marker)`：公开且已审核。
pub fn is_public_approved(row: &MarkerRow) -> bool {
    !row.deactivated && row.is_public && row.review_status == "APPROVED"
}

/// `canView(marker, viewer)` 的扩展边界。
///
/// - viewer 非空且未删除，且为 `ADMIN` → true；
/// - viewer 的 `publicId` 等于点位 `userPublicId` → true；
/// - 否则等于 [`is_public_approved`]。
pub fn can_view(row: &MarkerRow, viewer: Option<&Viewer<'_>>) -> bool {
    if let Some(viewer) = viewer
        && !viewer.deleted
    {
        if viewer.role.eq_ignore_ascii_case("ADMIN") {
            return true;
        }
        if row.deactivated {
            return false;
        }
        if let (Some(public_id), Some(owner)) = (viewer.public_id, row.user_public_id.as_deref())
            && public_id == owner
        {
            return true;
        }
    }
    is_public_approved(row)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MarkerPhoto {
    pub id: i64,
    #[serde(skip)]
    pub marker_id: i64,
    pub url: String,
    pub sort_order: i64,
}
