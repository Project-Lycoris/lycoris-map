//! 请求与响应 DTO。
//!
//! 请求字段全部为 `Option`，对齐 Java Jackson 缺失字段为 `null` 的行为；
//! `UserResponse` 不暴露 `id`/`role`/`password`。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::users::UserRow;

// 含密码/口令的请求体不派生 Debug，避免后续诊断整份输出敏感值。
#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    #[serde(rename = "verificationCode")]
    pub verification_code: Option<String>,
    pub username: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
    /// 蜜罐字段：非空即视为无效注册。
    pub website: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub nickname: Option<String>,
    pub pronouns: Option<String>,
    pub signature: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordRequest {
    pub old_password: Option<String>,
    pub new_password: Option<String>,
}

/// AuthController 成功体的用户对象（7 字段）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub public_id: String,
    pub username: Option<String>,
    pub nickname: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub pronouns: Option<String>,
    pub signature: Option<String>,
}

impl From<&UserRow> for UserResponse {
    fn from(user: &UserRow) -> Self {
        Self {
            public_id: user.public_id.to_string(),
            username: user.username.clone(),
            nickname: user.nickname.clone(),
            email: user.email.clone(),
            avatar_url: user.avatar_url.clone(),
            pronouns: user.pronouns.clone(),
            signature: user.signature.clone(),
        }
    }
}

pub fn user_response(user: &UserRow) -> Value {
    serde_json::to_value(UserResponse::from(user)).unwrap_or(Value::Null)
}

/// 管理员列表项（含 `id`/`role`/`deleted`/`deletedAt`）。
pub fn admin_user_item(user: &UserRow) -> Value {
    json!({
        "id": user.id,
        "publicId": user.public_id.to_string(),
        "username": user.username,
        "nickname": user.nickname,
        "email": user.email,
        "avatarUrl": user.avatar_url,
        "pronouns": user.pronouns,
        "signature": user.signature,
        "role": user.role,
        "deleted": user.deleted,
        "deletedAt": user.deleted_at,
    })
}
