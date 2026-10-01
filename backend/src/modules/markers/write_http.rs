//! 阶段 3 点位写入/收藏/审核 HTTP 层：薄 handler、认证提取器与响应本地化。
//!
//! 本模块不含业务事务与 SQL：身份由 [`CurrentUser`]/[`AdminUser`] 提取后，
//! 只从 `Identity.user` 当前数据库行构造 [`Actor`]（绝不从请求体填 owner/admin），再调用
//! 已验收的 [`MarkerWriteService`]。成功 `MarkerRow`/`Vec<MarkerRow>` 一律经
//! [`MarkerService::localize`] 生成与读取接口同形的 26 字段响应并带语言 `Vary`，
//! 不直接序列化数据库行。
//!
//! User endpoints require `CurrentUser`; every admin endpoint requires the
//! current database role ADMIN through `AdminUser`. No secondary password.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::{AdminUser, CurrentUser, Identity};
use crate::error::{ApiError, ErrorShape};
use crate::modules::markers::http::{json_marker, json_markers};
use crate::modules::markers::localization;
use crate::modules::markers::model::MarkerRow;
use crate::modules::markers::write_model::{
    Actor, EditProposalDto, MarkerCreateRequest, MarkerUpdateRequest, WriteError,
};
use crate::web::{self, MarkerJsonBody};

/// 构建点位写入/收藏/审核路由。静态段优先于 `/{id}`，不会互相遮蔽。
pub fn router() -> Router<AppState> {
    Router::new()
        // 用户写与本人读取（登录）。
        .route("/api/markers", post(create))
        .route(
            "/api/markers/{id}",
            patch(update_marker).delete(delete_marker),
        )
        .route(
            "/api/markers/{id}/favorite",
            post(add_favorite).delete(remove_favorite),
        )
        .route("/api/markers/me/favorites", get(my_favorites))
        .route("/api/markers/me/created", get(my_created))
        .route(
            "/api/markers/me/favorites/details",
            get(my_favorite_details),
        )
        // 管理员诊断读：只要求当前数据库角色为 ADMIN，不要求二次验证。
        .route("/api/markers/all", get(list_all))
        // Current database role ADMIN.
        .route("/api/admin/markers/pending", get(admin_pending))
        .route("/api/admin/markers/pending-edits", get(admin_pending_edits))
        .route("/api/admin/markers/all", get(admin_all))
        .route("/api/admin/markers/{id}/approve", post(admin_approve))
        .route("/api/admin/markers/{id}/reject", post(admin_reject))
        .route("/api/admin/markers/{id}/restore", post(admin_restore))
        .route(
            "/api/admin/markers/{id}",
            patch(admin_update).delete(admin_delete),
        )
        .route(
            "/api/admin/markers/edit-proposals/{id}/approve",
            post(admin_approve_proposal),
        )
        .route(
            "/api/admin/markers/edit-proposals/{id}/reject",
            post(admin_reject_proposal),
        )
}

#[derive(Debug, Deserialize)]
struct LangQuery {
    lang: Option<String>,
}

/// 只从当前数据库账号构造身份快照，`public_id` 为归属依据、`username` 仅作快照。
fn actor_of(identity: &Identity) -> Actor {
    Actor::new(
        identity.user.public_id.to_string(),
        identity.user.username_or_empty().to_string(),
        identity.is_admin(),
    )
}

fn response_language(query: &LangQuery, headers: &HeaderMap) -> &'static str {
    localization::for_read(query.lang.as_deref(), headers)
}

fn write_error(error: WriteError) -> Response {
    error.into_reply(ErrorShape::Text).into_response()
}

fn api_text(error: ApiError) -> Response {
    error.into_reply(ErrorShape::Text).into_response()
}

fn empty_ok() -> Response {
    web::empty(StatusCode::OK)
}

/// 把单行本地化为 26 字段响应（带语言 `Vary`）。
async fn localize_one(state: &AppState, row: MarkerRow, lang: &'static str) -> Response {
    json_marker(&state.markers.committed_row(row, lang).await)
}

/// 把多行本地化为 26 字段响应数组（带语言 `Vary`）。
async fn localize_many(state: &AppState, rows: Vec<MarkerRow>, lang: &'static str) -> Response {
    match state.markers.localize(rows, lang).await {
        Ok(dtos) => json_markers(&dtos),
        Err(error) => api_text(error),
    }
}

/// POST /api/markers
async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerCreateRequest>,
) -> Response {
    let actor = actor_of(&user.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .create_marker(&actor, request_language, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// PATCH /api/markers/{id}：普通用户只创建待审编辑提案并返回未修改的原点位。
async fn update_marker(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerUpdateRequest>,
) -> Response {
    let actor = actor_of(&user.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .create_edit_proposal(&actor, request_language, id, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// DELETE /api/markers/{id}
async fn delete_marker(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .delete_owned_marker(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/markers/{id}/favorite
async fn add_favorite(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .add_favorite(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// DELETE /api/markers/{id}/favorite
async fn remove_favorite(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .remove_favorite(&actor_of(&user.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/favorites：可见收藏点位 ID 列表。
async fn my_favorites(State(state): State<AppState>, user: CurrentUser) -> Response {
    match state.markers_write.favorite_ids(&actor_of(&user.0)).await {
        Ok(ids) => Json(ids).into_response(),
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/created
async fn my_created(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state.markers_write.list_created(&actor_of(&user.0)).await {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/me/favorites/details
async fn my_favorite_details(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .favorite_markers(&actor_of(&user.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/markers/all：管理员全部点位读取，不要求二次验证。
async fn list_all(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .list_all_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/pending
async fn admin_pending(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .pending_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/pending-edits：普通 JSON，19 字段提案 DTO。
async fn admin_pending_edits(State(state): State<AppState>, admin: AdminUser) -> Response {
    match state
        .markers_write
        .pending_edit_proposals(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let items: Vec<EditProposalDto> = rows.into_iter().map(EditProposalDto::from).collect();
            Json(items).into_response()
        }
        Err(error) => write_error(error),
    }
}

/// GET /api/admin/markers/all
async fn admin_all(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .list_all_markers(&actor_of(&admin.0))
        .await
    {
        Ok(rows) => {
            let lang = response_language(&query, &headers);
            localize_many(&state, rows, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/{id}/approve
async fn admin_approve(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .approve_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/{id}/reject
async fn admin_reject(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .reject_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// PATCH /api/admin/markers/{id}：管理员直接编辑，不生成提案。
async fn admin_update(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
    MarkerJsonBody(request): MarkerJsonBody<MarkerUpdateRequest>,
) -> Response {
    let actor = actor_of(&admin.0);
    let request_language = localization::from_headers(&headers);
    match state
        .markers_write
        .admin_update_marker(&actor, request_language, id, request)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// DELETE /api/admin/markers/{id}
async fn admin_delete(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .admin_delete_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/{id}/restore: restore without approving or changing visibility.
async fn admin_restore(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .admin_restore_marker(&actor_of(&admin.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/edit-proposals/{id}/approve
async fn admin_approve_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<LangQuery>,
) -> Response {
    match state
        .markers_write
        .approve_edit_proposal(&actor_of(&admin.0), id)
        .await
    {
        Ok(row) => {
            let lang = response_language(&query, &headers);
            localize_one(&state, row, lang).await
        }
        Err(error) => write_error(error),
    }
}

/// POST /api/admin/markers/edit-proposals/{id}/reject
async fn admin_reject_proposal(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Response {
    match state
        .markers_write
        .reject_edit_proposal(&actor_of(&admin.0), id)
        .await
    {
        Ok(()) => empty_ok(),
        Err(error) => write_error(error),
    }
}
