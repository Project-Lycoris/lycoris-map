//! AdminAuthController / AdminUserController 对应接口。
//!
//! Access requires the current database role ADMIN; mutations retain optimistic locking.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::auth::AdminUser;
use crate::dto::admin_user_item;
use crate::users;
use crate::web;

/// Compatibility endpoint for cached clients. No passcode or extra session
/// state is required; the extractor still checks the current database role.
pub async fn verify(_admin: AdminUser) -> Response {
    web::empty(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct AdminUsersQuery {
    pub page: Option<i64>,
    pub size: Option<i64>,
    pub q: Option<String>,
}

/// GET /api/admin/users
pub async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(query): Query<AdminUsersQuery>,
) -> Response {
    let page = query.page.unwrap_or(0).max(0);
    let size = query.size.unwrap_or(10).clamp(1, 100);
    let q = query.q.unwrap_or_default().trim().to_string();

    match users::search_for_admin(&state.db, &q, page, size).await {
        Ok((total, rows)) => {
            let total_pages = if total == 0 {
                0
            } else {
                (total + size - 1) / size
            };
            let items: Vec<serde_json::Value> = rows.iter().map(admin_user_item).collect();
            web::json(
                StatusCode::OK,
                json!({
                    "page": page,
                    "size": size,
                    "totalPages": total_pages,
                    "totalElements": total,
                    "items": items,
                }),
            )
        }
        Err(_) => web::unavailable(),
    }
}

/// POST /api/admin/users/{id}/reset-password
pub async fn reset_password(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i32>,
) -> Response {
    let user = match users::find_any_by_id(&state.db, id).await {
        Ok(Some(user)) => user,
        Ok(None) => return web::text(StatusCode::NOT_FOUND, "用户不存在"),
        Err(_) => return web::unavailable(),
    };
    if user.deleted {
        return web::text(StatusCode::BAD_REQUEST, "已删除用户不能重置密码");
    }

    let hash = match state
        .passwords
        .hash(state.config.admin_default_user_password.clone())
        .await
    {
        Ok(hash) => hash,
        Err(_) => return web::unavailable(),
    };

    match users::reset_password(&state.db, id, user.row_version, &hash).await {
        Ok(Some(_)) => web::json(
            StatusCode::OK,
            json!({ "message": "密码已重置为默认密码", "username": user.username }),
        ),
        Ok(None) => optimistic_conflict(),
        Err(_) => web::unavailable(),
    }
}

/// DELETE /api/admin/users/{id}
pub async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i32>,
) -> Response {
    if admin.0.user.id == id {
        return web::text(StatusCode::BAD_REQUEST, "不能删除当前登录管理员账号");
    }
    let user = match users::find_any_by_id(&state.db, id).await {
        Ok(Some(user)) => user,
        Ok(None) => return web::text(StatusCode::NOT_FOUND, "用户不存在"),
        Err(_) => return web::unavailable(),
    };
    if user.deleted {
        return web::text(StatusCode::NOT_FOUND, "用户不存在");
    }

    match users::soft_delete(&state.db, id, user.row_version).await {
        Ok(true) => web::json(StatusCode::OK, json!({ "message": "用户已删除" })),
        Ok(false) => optimistic_conflict(),
        Err(_) => web::unavailable(),
    }
}

/// POST /api/admin/users/{id}/restore
pub async fn restore_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i32>,
) -> Response {
    let user = match users::find_any_by_id(&state.db, id).await {
        Ok(Some(user)) => user,
        Ok(None) => return web::text(StatusCode::NOT_FOUND, "用户不存在"),
        Err(_) => return web::unavailable(),
    };
    if !user.deleted {
        return web::text(StatusCode::BAD_REQUEST, "该用户未被删除");
    }

    match users::restore(&state.db, id, user.row_version).await {
        Ok(true) => web::json(StatusCode::OK, json!({ "message": "用户已恢复" })),
        Ok(false) => optimistic_conflict(),
        Err(_) => web::unavailable(),
    }
}

fn optimistic_conflict() -> Response {
    web::api_error(StatusCode::CONFLICT, 409, "数据已更新，请刷新后重试")
}
