//! Image moderation routes and the retired cleanup endpoint.
//!
//! 全部要求 [`AdminUser`]：未登录 → 安全入口固定 401 JSON；已认证非管理员 → Spring Boot
//! 默认 403 JSON。业务逻辑全部交给已验收的
//! [`MediaService`](crate::media::MediaService)（事务/授权/缓存）与
//! [`MarkerService`](crate::modules::markers::service::MarkerService) 本地化，**不在 handler
//! 手写 SQL 或二次更新**。成功体：审批为普通 JSON 本地化 `MarkerDto`（带 `Vary`）、
//! 待审清单为普通 JSON 数组、驳回为 200 空体、旧清理入口为 410；
//! 业务错误为中文纯文本。

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::{AdminUser, Identity};
use crate::error::ErrorShape;
use crate::modules::markers::http::json_marker;
use crate::modules::markers::localization;
use crate::modules::markers::model::Viewer;
use crate::multipart::media_error_response;
use crate::web;

#[derive(Debug, Deserialize)]
pub struct LangOnly {
    lang: Option<String>,
}

/// 由数据库当前账号构造可信 `Viewer`；`public_id` 为调用方持有的生命周期覆盖整个请求。
fn viewer_of<'a>(public_id: &'a str, identity: &'a Identity) -> Viewer<'a> {
    Viewer {
        public_id: Some(public_id),
        role: identity.user.role.as_str(),
        deleted: identity.user.deleted,
    }
}

/// GET /api/admin/markers/pending-images
pub async fn pending_images(State(state): State<AppState>, admin: AdminUser) -> Response {
    let admin = admin.0;
    let public_id = admin.user.public_id.to_string();
    let viewer = viewer_of(&public_id, &admin);
    match state.media.list_pending_images(&viewer).await {
        Ok(items) => web::json(StatusCode::OK, web::to_json(&items)),
        Err(error) => media_error_response(error, ErrorShape::Text),
    }
}

/// POST /api/admin/markers/image-proposals/{id}/approve
pub async fn approve_image_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(params): Query<LangOnly>,
) -> Response {
    let lang = localization::for_read(params.lang.as_deref(), &headers);
    let admin = admin.0;
    let public_id = admin.user.public_id.to_string();
    let viewer = viewer_of(&public_id, &admin);
    let reviewer = admin.user.username_or_empty();

    match state
        .media
        .approve_image_proposal(id, &viewer, reviewer)
        .await
    {
        Ok(row) => json_marker(&state.markers.committed_row(row, lang).await),
        Err(error) => media_error_response(error, ErrorShape::Text),
    }
}

/// POST /api/admin/markers/image-proposals/{id}/reject
pub async fn reject_image_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    let admin = admin.0;
    let public_id = admin.user.public_id.to_string();
    let viewer = viewer_of(&public_id, &admin);
    let reviewer = admin.user.username_or_empty();

    match state
        .media
        .reject_image_proposal(id, &viewer, reviewer)
        .await
    {
        Ok(()) => web::empty(StatusCode::OK),
        Err(error) => media_error_response(error, ErrorShape::Text),
    }
}

/// POST /api/admin/markers/cleanup-missing-images
pub async fn cleanup_missing_images(_admin: AdminUser) -> Response {
    // Retire the old destructive entry point as well as its UI. Cached clients
    // must never clear stored references based on a transient storage failure.
    web::text(StatusCode::GONE, "批量清理功能已停用")
}
