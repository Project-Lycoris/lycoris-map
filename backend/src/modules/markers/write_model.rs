//! 阶段 3 写入核心的数据模型、请求 DTO、数据库行与局部错误。
//!
//! 事务与模型独立于路由/认证：身份由调用方以显式 [`Actor`] 传入，可见性复用读取层
//! [`Viewer`]；认证、二次验证、路由与 `AppState` 由后续薄 handler 负责。[`WriteError`] 只描述
//! 状态与契约中文，并提供到 `ApiReply` 的映射，保留“点位不存在”“编辑提案不存在”等具体提示，
//! 不依赖或重做全局错误架构。

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{ApiReply, ApiResponse, ErrorShape};
use crate::modules::markers::model::Viewer;

/// 契约错误文案（与 Java 控制器/服务逐字一致）。
pub const MSG_MARKER_NOT_FOUND: &str = "点位不存在";
pub const MSG_FORBIDDEN: &str = "无权限";
pub const MSG_PROPOSAL_NOT_FOUND: &str = "编辑提案不存在";
pub const MSG_RELATED_MARKER_NOT_FOUND: &str = "关联点位不存在";
pub const MSG_PROPOSAL_ALREADY_HANDLED: &str = "该提案已处理";
pub const MSG_STALE_VERSION: &str = "点位已更新或提案缺少版本信息，请按最新内容重新提交后审核";
/// 新建点位的 `markImage` 只允许 null/空白；图片必须经上传提案与审核关联。
pub const MSG_MARK_IMAGE_UPLOAD_ONLY: &str = "markImage 只能为空，请通过图片上传提交";
/// 非无障碍卫生间类别携带场所标签；或标签值不在白名单内。
pub const MSG_VENUE_TYPE_INVALID: &str = "venueType 不合法";

/// 受控场所标签取值（`accessible_toilet` 专属，与最新场所标签迁移的 CHECK 一致）。
pub const VENUE_TYPES: [&str; 8] = [
    "metro",
    "hospital",
    "mall",
    "railway_station",
    "school",
    "public_toilet",
    "airport",
    "other",
];

/// 新建无障碍卫生间未显式传标签时的默认值。
pub const DEFAULT_VENUE_TYPE: &str = "other";

/// 已认证身份。
///
/// 由后续 HTTP 层从会话构造；核心不读取会话、不在 `Actor` 上伪造二级验证状态。
/// `public_id` 是归属依据，`username` 仅作快照。
#[derive(Debug, Clone)]
pub struct Actor {
    pub public_id: String,
    pub username: String,
    pub is_admin: bool,
}

impl Actor {
    pub fn new(public_id: impl Into<String>, username: impl Into<String>, is_admin: bool) -> Self {
        Self {
            public_id: public_id.into(),
            username: username.into(),
            is_admin,
        }
    }

    /// 复用读取层可见性判定：管理员或属主可见，否则仅“公开且已审核”可见。
    pub fn viewer(&self) -> Viewer<'_> {
        Viewer {
            public_id: Some(&self.public_id),
            role: if self.is_admin { "ADMIN" } else { "USER" },
            deleted: false,
        }
    }
}

/// `POST /api/markers` 请求体，字段与 Java `MarkerCreateRequest` 一致。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkerCreateRequest {
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub category: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub is_public: Option<bool>,
    pub is_active: Option<bool>,
    pub open_time_start: Option<String>,
    pub open_time_end: Option<String>,
    pub client_request_id: Option<String>,
    pub mark_image: Option<String>,
    /// 场所标签；仅 `category == accessible_toilet` 允许，缺省为 `other`。
    pub venue_type: Option<String>,
}

/// `PATCH /api/markers/{id}` 与管理员 PATCH 请求体，字段与 Java `MarkerUpdateRequest` 一致。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkerUpdateRequest {
    pub category: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub is_public: Option<bool>,
    pub is_active: Option<bool>,
    pub open_time_start: Option<String>,
    pub open_time_end: Option<String>,
    /// 场所标签；语义见 [`MarkerCreateRequest::venue_type`]。
    pub venue_type: Option<String>,
}

/// `marker_edit_proposals` 中与审核/列表相关的列。
///
/// `marker_title`/`marker_lat`/`marker_lng` 是创建提案时的点位快照；审核响应里的 `lat`/`lng`
/// 对外映射为快照坐标。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EditProposalRow {
    pub id: i64,
    pub marker_id: i64,
    pub marker_title: String,
    pub marker_lat: f64,
    pub marker_lng: f64,
    pub category: String,
    pub title: String,
    pub description: Option<String>,
    pub language: String,
    pub is_public: bool,
    pub is_active: bool,
    pub open_time_start: Option<String>,
    pub open_time_end: Option<String>,
    pub venue_type: Option<String>,
    pub proposer_username: String,
    pub proposer_public_id: Option<String>,
    pub proposer_is_owner: bool,
    pub status: String,
    pub base_marker_version: Option<i64>,
    pub base_content: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// `GET /api/admin/markers/pending-edits` 的单条响应，字段与 Java 控制器输出一致。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditProposalDto {
    pub id: i64,
    pub marker_id: i64,
    pub marker_title: String,
    pub lat: f64,
    pub lng: f64,
    pub category: String,
    pub title: String,
    pub description: Option<String>,
    pub language: String,
    pub is_public: bool,
    pub is_active: bool,
    pub open_time_start: Option<String>,
    pub open_time_end: Option<String>,
    pub venue_type: Option<String>,
    pub proposer_username: String,
    pub proposer_public_id: Option<String>,
    pub proposer_is_owner: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

impl From<EditProposalRow> for EditProposalDto {
    fn from(row: EditProposalRow) -> Self {
        Self {
            id: row.id,
            marker_id: row.marker_id,
            marker_title: row.marker_title,
            lat: row.marker_lat,
            lng: row.marker_lng,
            category: row.category,
            title: row.title,
            description: row.description,
            language: row.language,
            is_public: row.is_public,
            is_active: row.is_active,
            open_time_start: row.open_time_start,
            open_time_end: row.open_time_end,
            venue_type: row.venue_type,
            proposer_username: row.proposer_username,
            proposer_public_id: row.proposer_public_id,
            proposer_is_owner: row.proposer_is_owner,
            status: row.status,
            created_at: row.created_at,
        }
    }
}

/// 写入核心的业务错误；状态与文案区分契约所需分支，不把任何数据库错误当作幂等命中。
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("{0}")]
    BadRequest(String),
    #[error("无权限")]
    Forbidden,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    /// 数据库语句/锁等待超时（SQLSTATE 57014/55P03）；事务已回滚，受控映射 503。
    #[error("服务暂时不可用")]
    Unavailable,
    /// 数据库/未知内部错误；日志已做受控摘要，不携带 SQL 或参数。
    #[error("服务器内部错误")]
    Internal,
}

impl WriteError {
    pub fn status(&self) -> StatusCode {
        match self {
            WriteError::BadRequest(_) => StatusCode::BAD_REQUEST,
            WriteError::Forbidden => StatusCode::FORBIDDEN,
            WriteError::NotFound(_) => StatusCode::NOT_FOUND,
            WriteError::Conflict(_) => StatusCode::CONFLICT,
            WriteError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            WriteError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// 按接口契约选择响应形状；点位/提案错误默认使用中文纯文本。
    pub fn into_reply(self, shape: ErrorShape) -> ApiReply {
        let status = self.status();
        let message = self.to_string();
        match shape {
            ErrorShape::ApiResponse => ApiReply::api(
                status,
                ApiResponse::error(i32::from(status.as_u16()), message),
            ),
            ErrorShape::Text => ApiReply::text(status, message),
            ErrorShape::Empty => ApiReply::empty(status),
        }
    }
}

/// 校验用：数据库 `varchar(n)` 按字符计数（与 PostgreSQL `char_length` 一致）。
pub fn db_len(value: &str) -> usize {
    value.chars().count()
}

/// 校验用：与 Java `String.length()` 一致的 UTF-16 计数，仅用于 Java 显式做长度校验的字段。
pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}
