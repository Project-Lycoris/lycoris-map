//! 媒体业务编排：头像引用/更新、受控 `/uploads` 读取、点位图片提案与清理。
//!
//! 职责边界：
//!
//! - **身份来源由调用方提供**。HTTP 层负责会话认证与管理二次验证，随后把可信的
//!   `Viewer`（公共 ID / 角色 / 删除标记）、用户名、用户 ID 与行版本传进来。本层
//!   **不**解析 Cookie、**不**信任请求字段、**不**另立身份系统，只做资源级授权
//!   （属主 / 管理员 / 可见性）与业务一致性校验。
//! - **可见性复用** `modules::markers::model::can_view`：私有/未审核点位对非属主一律
//!   按不存在处理（404），不泄露存在性。
//! - **文件生命周期**：只有 [`ImageStore`] 保存成功并返回 URL 后才允许写入数据库引用。
//!   保存失败绝不返回可引用 URL；数据库写入失败或事务结果不确定时保留本次新建的孤立
//!   文件（不 `Drop` 时删除可能已被引用的文件），跨 DB+FS 的原子性不在本轮承诺内。
//! - **缓存**：仅在 PG 提交成功后调用 `MarkerCache::invalidate`，并额外用 500ms 超时
//!   包住（缓存内部修复另行推进），Redis 故障或超时只记录受控日志，绝不反转已提交结果。
//! - 事务顺序：图片审批先锁提案（`PENDING`）、再锁/更新点位，然后写提案审核人与时间，
//!   最后提交；任一步失败整体回滚。

use std::time::Duration;

use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;

use crate::media::model::{CleanupResult, PendingImageItem};
use crate::media::repository::MediaRepository;
use crate::media::storage::{ImageStore, MediaDirectory, MediaError, OpenedImage};
use crate::modules::markers::cache::MarkerCache;
use crate::modules::markers::model::{MarkerRow, Viewer, can_view};

/// 提交后缓存失效的最长等待；超时按“缓存不可用”处理，不影响已提交写入。
const CACHE_INVALIDATE_TIMEOUT: Duration = Duration::from_millis(500);
const UPLOADS_PREFIX: &str = "/uploads/";
const MARKERS_PREFIX: &str = "/uploads/markers/";

/// 媒体业务错误。只描述结果，响应形状（`ApiResponse` / 纯文本 / 空体）由 HTTP 层选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaServiceError {
    /// 未登录（缺少可信身份）。
    Unauthorized,
    /// 已识别身份但无管理员权限。
    Forbidden,
    /// 资源按不存在处理；携带对外中文消息（如“图片提案不存在”）。
    NotFound(&'static str),
    /// 请求不合法；携带对外中文消息。
    BadRequest(String),
    /// 并发/状态冲突；携带对外中文消息。
    Conflict(String),
    /// 请求体过大。
    PayloadTooLarge,
    /// 图片处理繁忙或依赖暂不可用。
    Unavailable,
    /// 服务器内部错误（已记录底层原因）。
    Internal,
}

impl MediaServiceError {
    /// 对外 HTTP 状态。
    pub fn status(&self) -> StatusCode {
        match self {
            MediaServiceError::Unauthorized => StatusCode::UNAUTHORIZED,
            MediaServiceError::Forbidden => StatusCode::FORBIDDEN,
            MediaServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            MediaServiceError::BadRequest(_) => StatusCode::BAD_REQUEST,
            MediaServiceError::Conflict(_) => StatusCode::CONFLICT,
            MediaServiceError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            MediaServiceError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            MediaServiceError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// 对外中文消息。
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// HTTP 层可用此显式分支映射 413，而不必靠字符串匹配“文件过大”。
    pub fn is_payload_too_large(&self) -> bool {
        matches!(self, MediaServiceError::PayloadTooLarge)
    }
}

impl std::fmt::Display for MediaServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            MediaServiceError::Unauthorized => "未登录",
            MediaServiceError::Forbidden => "无权限",
            MediaServiceError::NotFound(message) => message,
            MediaServiceError::BadRequest(message) => message,
            MediaServiceError::Conflict(message) => message,
            MediaServiceError::PayloadTooLarge => "上传文件过大，请选择 5MB 以内的图片",
            MediaServiceError::Unavailable => "服务暂时不可用",
            MediaServiceError::Internal => "服务器内部错误",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MediaServiceError {}

/// 条件头像更新的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvatarUpdateOutcome {
    /// 写入成功并推进了行版本。
    Updated {
        avatar_url: String,
        row_version: i64,
    },
    /// 用户不存在或已软删除。
    NotFound,
    /// 行版本不匹配（并发更新），未写入。
    VersionConflict,
}

/// 媒体业务服务。构造只依赖 `PgPool`、`ImageStore` 与 `MarkerCache`。
#[derive(Clone)]
pub struct MediaService {
    repo: MediaRepository,
    store: ImageStore,
    cache: MarkerCache,
}

impl MediaService {
    pub fn new(pool: PgPool, store: ImageStore, cache: MarkerCache) -> Self {
        Self {
            repo: MediaRepository::new(pool),
            store,
            cache,
        }
    }

    /// 图片存储核心（供 HTTP 层发送时复用，不额外读入内存）。
    pub fn store(&self) -> &ImageStore {
        &self.store
    }

    /// 按公共 ID 读取未删除用户的头像 URL。
    ///
    /// 公共 ID 非法、用户不存在/已删除、无头像或存储值不是合法的
    /// `/uploads/avatars/<文件名>` 路径时返回 `None`（对外 404）。
    pub async fn avatar_url_by_public_id(
        &self,
        public_id: &str,
    ) -> Result<Option<String>, MediaServiceError> {
        let Ok(uuid) = Uuid::parse_str(public_id.trim()) else {
            return Ok(None);
        };
        let Some(url) = self
            .repo
            .avatar_url_by_public_id(uuid)
            .await
            .map_err(|error| db_error("读取用户头像引用", error))?
        else {
            return Ok(None);
        };
        Ok(sanitize_avatar_url(url))
    }

    /// 按内部用户 ID 读取未删除用户的头像 URL（`GET /api/me/avatar`）。
    pub async fn avatar_url_by_user_id(
        &self,
        user_id: i32,
    ) -> Result<Option<String>, MediaServiceError> {
        let Some(url) = self
            .repo
            .avatar_url_by_user_id(user_id)
            .await
            .map_err(|error| db_error("读取当前用户头像引用", error))?
        else {
            return Ok(None);
        };
        Ok(sanitize_avatar_url(url))
    }

    /// 保存新头像并条件更新当前用户。
    ///
    /// 顺序：预检用户状态（缺失/已删除 → `NotFound`，行版本不符 → `VersionConflict`，均在
    /// 落盘前完成）→ `ImageStore::save` → 同事务内以 `id + deleted=false + row_version`
    /// 条件更新 `avatar_url` 与 `row_version`，**不覆盖资料其他列**。
    ///
    /// 条件更新未命中（预检后被并发删除/改版本）返回 `NotFound`/`VersionConflict`，绝不
    /// 误报成功；本次新建的图片文件保留为可审计的孤立文件，不自动删除旧头像文件。
    pub async fn upload_avatar(
        &self,
        user_id: i32,
        expected_row_version: i64,
        caller_public_id: &str,
        bytes: Vec<u8>,
    ) -> Result<AvatarUpdateOutcome, MediaServiceError> {
        match self
            .repo
            .user_avatar_status(user_id)
            .await
            .map_err(|error| db_error("预检用户头像状态", error))?
        {
            None => return Ok(AvatarUpdateOutcome::NotFound),
            Some(status) if status.deleted => return Ok(AvatarUpdateOutcome::NotFound),
            Some(status) if status.row_version != expected_row_version => {
                return Ok(AvatarUpdateOutcome::VersionConflict);
            }
            Some(_) => {}
        }

        let prefix = format!("avatar-{caller_public_id}");
        let stored = self
            .store
            .save(MediaDirectory::Avatars, &prefix, bytes)
            .await
            .map_err(upload_error)?;

        let mut transaction = self
            .repo
            .pool()
            .begin()
            .await
            .map_err(|error| db_error("开启头像更新事务", error))?;
        let updated = self
            .repo
            .update_avatar_conditional(&mut transaction, user_id, expected_row_version, &stored.url)
            .await
            .map_err(|error| db_error("条件更新用户头像", error))?;

        match updated {
            Some(row) => {
                transaction
                    .commit()
                    .await
                    .map_err(|error| db_error("提交头像更新事务", error))?;
                Ok(AvatarUpdateOutcome::Updated {
                    avatar_url: row.avatar_url.unwrap_or_else(|| stored.url.clone()),
                    row_version: row.row_version,
                })
            }
            None => {
                drop(transaction);
                let outcome = match self
                    .repo
                    .user_avatar_status(user_id)
                    .await
                    .map_err(|error| db_error("复核用户头像状态", error))?
                {
                    Some(status) if !status.deleted => AvatarUpdateOutcome::VersionConflict,
                    _ => AvatarUpdateOutcome::NotFound,
                };
                Ok(outcome)
            }
        }
    }

    /// 受控 `/uploads/{directory}/{filename}` 读取。
    ///
    /// - `avatars`：匿名可读（文件安全与存在性仍由 [`ImageStore::open`] 校验）。
    /// - `markers`：先做资源级授权，再打开真正的流式文件句柄；不可见/非法/不存在一律
    ///   返回 `NotFound`，不泄露存在性，也不在此伪造 HTTP 路由。
    pub async fn open_uploads(
        &self,
        directory: &str,
        filename: &str,
        viewer: Option<&Viewer<'_>>,
    ) -> Result<OpenedImage, MediaServiceError> {
        let Some(media_directory) = MediaDirectory::parse(directory) else {
            return Err(MediaServiceError::NotFound("图片不存在"));
        };
        if media_directory == MediaDirectory::Markers {
            let url = format!("{UPLOADS_PREFIX}{directory}/{filename}");
            if !self.marker_image_authorized(&url, viewer).await? {
                return Err(MediaServiceError::NotFound("图片不存在"));
            }
        }
        self.store
            .open(directory, filename)
            .await
            .map_err(read_error)
    }

    /// 提交点位图片提案（`POST /api/markers/{id}/image`）。
    ///
    /// 先做不落盘的可见性检查，避免为不可见点位白白解码；`ImageStore` 保存成功后才在同一
    /// 事务内重新锁定点位并再次校验可见性，二者都通过才插入 `PENDING` 提案。点位本身
    /// **不更新**，返回锁定时的原 [`MarkerRow`] 供 HTTP 本地化。事务内第二次校验若失败，
    /// 本次新文件保留为孤立文件。
    pub async fn submit_marker_image(
        &self,
        marker_id: i64,
        viewer: Option<&Viewer<'_>>,
        caller_username: &str,
        caller_public_id: &str,
        bytes: Vec<u8>,
    ) -> Result<MarkerRow, MediaServiceError> {
        if caller_public_id.trim().is_empty() {
            return Err(MediaServiceError::Unauthorized);
        }
        if bytes.is_empty() {
            return Err(MediaServiceError::BadRequest("文件为空".to_string()));
        }

        // 廉价检查：可见性不通过时直接 404，不进入解码与落盘。
        let visible = self
            .repo
            .marker_by_id(marker_id)
            .await
            .map_err(|error| db_error("预检点位可见性", error))?;
        if visible.filter(|row| can_view(row, viewer)).is_none() {
            return Err(MediaServiceError::NotFound("点位不存在"));
        }

        let prefix = format!("proposal-marker-{marker_id}");
        let stored = self
            .store
            .save(MediaDirectory::Markers, &prefix, bytes)
            .await
            .map_err(upload_error)?;

        let mut transaction = self
            .repo
            .pool()
            .begin()
            .await
            .map_err(|error| db_error("开启图片提案事务", error))?;
        let Some(locked) = self
            .repo
            .lock_marker_by_id(&mut transaction, marker_id)
            .await
            .map_err(|error| db_error("锁定点位", error))?
        else {
            return Err(MediaServiceError::NotFound("点位不存在"));
        };
        if !can_view(&locked, viewer) {
            return Err(MediaServiceError::NotFound("点位不存在"));
        }
        self.repo
            .insert_image_proposal(
                &mut transaction,
                marker_id,
                &locked.title,
                caller_username,
                Some(caller_public_id),
                &stored.url,
            )
            .await
            .map_err(|error| db_error("插入图片提案", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| db_error("提交图片提案事务", error))?;
        Ok(locked)
    }

    /// 管理员待审图片提案清单（`PENDING`，`createdAt DESC`，契约 8 字段）。
    pub async fn list_pending_images(
        &self,
        viewer: &Viewer<'_>,
    ) -> Result<Vec<PendingImageItem>, MediaServiceError> {
        require_admin(viewer)?;
        self.repo
            .pending_images()
            .await
            .map_err(|error| db_error("查询待审图片提案", error))
    }

    /// 审批图片提案：同事务锁提案（`PENDING`）、锁并更新关联点位、写审核信息后提交。
    ///
    /// 不存在的关联点位返回 `NotFound("关联点位不存在")`，重复处理返回
    /// `BadRequest("该提案已处理")`。图片提案没有基准版本列，因此只推进 `version`，
    /// 不伪造 base version。提交成功后带超时地使点位缓存失效，Redis 故障不影响成功结果。
    pub async fn approve_image_proposal(
        &self,
        proposal_id: i64,
        viewer: &Viewer<'_>,
        reviewer_username: &str,
    ) -> Result<MarkerRow, MediaServiceError> {
        require_admin(viewer)?;
        let mut transaction = self
            .repo
            .pool()
            .begin()
            .await
            .map_err(|error| db_error("开启图片审批事务", error))?;

        let Some(proposal) = self
            .repo
            .lock_image_proposal(&mut transaction, proposal_id)
            .await
            .map_err(|error| db_error("锁定图片提案", error))?
        else {
            return Err(MediaServiceError::NotFound("图片提案不存在"));
        };
        if !proposal.status.eq_ignore_ascii_case("PENDING") {
            return Err(MediaServiceError::BadRequest("该提案已处理".to_string()));
        }

        // Serialize album publication per place; cover order follows upload proposal IDs,
        // so approving photos in reverse order cannot change their intended order.
        let marker = self
            .repo
            .lock_marker_by_id(&mut transaction, proposal.marker_id)
            .await
            .map_err(|error| db_error("锁定相册点位", error))?;
        if marker.is_none_or(|marker| marker.deactivated) {
            return Err(MediaServiceError::NotFound("关联点位不存在"));
        }
        self.repo
            .append_marker_photo(
                &mut transaction,
                proposal.marker_id,
                &proposal.image_url,
                proposal.id,
            )
            .await
            .map_err(|error| db_error("追加点位图片", error))?;

        let Some(updated) = self
            .repo
            .update_marker_mark_image(&mut transaction, &proposal.image_url, proposal.marker_id)
            .await
            .map_err(|error| db_error("更新点位图片", error))?
        else {
            return Err(MediaServiceError::NotFound("关联点位不存在"));
        };

        let affected = self
            .repo
            .approve_image_proposal(&mut transaction, reviewer_username, proposal_id)
            .await
            .map_err(|error| db_error("更新图片提案状态", error))?;
        if affected != 1 {
            return Err(MediaServiceError::Conflict("该提案已处理".to_string()));
        }

        transaction
            .commit()
            .await
            .map_err(|error| db_error("提交图片审批事务", error))?;
        self.invalidate_marker_cache().await;
        Ok(updated)
    }

    /// 拒绝图片提案：同事务锁提案（`PENDING`）后一次性置为 `REJECTED`。
    ///
    /// 与审批不同，拒绝**不要求关联点位存在**，也不改动点位或缓存。
    pub async fn reject_image_proposal(
        &self,
        proposal_id: i64,
        viewer: &Viewer<'_>,
        reviewer_username: &str,
    ) -> Result<(), MediaServiceError> {
        require_admin(viewer)?;
        let mut transaction = self
            .repo
            .pool()
            .begin()
            .await
            .map_err(|error| db_error("开启图片拒绝事务", error))?;

        let Some(proposal) = self
            .repo
            .lock_image_proposal(&mut transaction, proposal_id)
            .await
            .map_err(|error| db_error("锁定图片提案", error))?
        else {
            return Err(MediaServiceError::NotFound("图片提案不存在"));
        };
        if !proposal.status.eq_ignore_ascii_case("PENDING") {
            return Err(MediaServiceError::BadRequest("该提案已处理".to_string()));
        }

        let affected = self
            .repo
            .reject_image_proposal(&mut transaction, reviewer_username, proposal_id)
            .await
            .map_err(|error| db_error("更新图片提案状态", error))?;
        if affected != 1 {
            return Err(MediaServiceError::Conflict("该提案已处理".to_string()));
        }

        transaction
            .commit()
            .await
            .map_err(|error| db_error("提交图片拒绝事务", error))?;
        Ok(())
    }

    /// 清理确实缺失/非普通文件的点位图片引用（`POST /api/admin/markers/cleanup-missing-images`）。
    ///
    /// 只检查 `/uploads/markers/` 前缀的引用；逐个处理、不长时间锁全表、**不删除任何文件**。
    /// 仍存在的普通文件保留；缺失或非普通文件才以 `id/version/mark_image` 同时匹配做条件
    /// 清空并推进 `version`，因此并发换上的新图不会被旧检查清掉。非法路径明确拒绝访问；
    /// 权限或临时 I/O 异常保守保留 URL，不当作缺失。统计字段与 Java 对齐。
    pub async fn cleanup_missing_images(
        &self,
        viewer: &Viewer<'_>,
    ) -> Result<CleanupResult, MediaServiceError> {
        require_admin(viewer)?;
        let references = self
            .repo
            .marker_images_for_cleanup()
            .await
            .map_err(|error| db_error("查询待清理图片引用", error))?;

        let mut checked: i64 = 0;
        let mut cleared: i64 = 0;
        for reference in references {
            checked += 1;
            // SQL 已限定前缀；空前缀与非法文件名按 Java/安全规则不清理。
            let Some(mark_image) = reference.mark_image.as_deref() else {
                continue;
            };
            let Some(filename) = mark_image.strip_prefix(MARKERS_PREFIX) else {
                continue;
            };
            if filename.is_empty() {
                continue;
            }

            match self.store.exists("markers", filename).await {
                Ok(true) => {}
                Ok(false) => match self
                    .clear_missing_reference(reference.id, reference.version, mark_image)
                    .await
                {
                    Ok(true) => {
                        cleared += 1;
                        // 每次提交后立即失效：中途失败时已提交的清理也必须反映到缓存。
                        self.invalidate_marker_cache().await;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        tracing::error!(
                            target: "lycoris_backend::media",
                            checked,
                            cleared,
                            "清理中途失败：已提交的引用清理保留且已失效缓存，未提交部分未生效"
                        );
                        return Err(error);
                    }
                },
                Err(MediaError::InvalidName) | Err(MediaError::InvalidDirectory) => {
                    tracing::warn!(
                        target: "lycoris_backend::media",
                        marker_id = reference.id,
                        "清理跳过非法点位图片路径，保留引用"
                    );
                }
                Err(MediaError::Io(error)) => {
                    tracing::warn!(
                        target: "lycoris_backend::media",
                        marker_id = reference.id,
                        io_kind = ?error.kind(),
                        "清理遇到 I/O 异常，保守保留图片引用"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        target: "lycoris_backend::media",
                        marker_id = reference.id,
                        error = %error,
                        "清理遇到临时错误，保守保留图片引用"
                    );
                }
            }
        }

        Ok(CleanupResult {
            checked,
            cleared,
            message: "失效图片链接清理完成".to_string(),
        })
    }

    /// 对单个缺失/非普通文件引用执行一次条件清空事务，返回是否实际清空。
    ///
    /// 每个引用独立事务提交，不做全表长事务；提交由调用方负责立即失效缓存。
    async fn clear_missing_reference(
        &self,
        marker_id: i64,
        version: i64,
        mark_image: &str,
    ) -> Result<bool, MediaServiceError> {
        let mut transaction = self
            .repo
            .pool()
            .begin()
            .await
            .map_err(|error| db_error("开启清理事务", error))?;
        let affected = self
            .repo
            .clear_mark_image_conditional(&mut transaction, marker_id, version, mark_image)
            .await
            .map_err(|error| db_error("条件清空图片引用", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| db_error("提交清理事务", error))?;
        Ok(affected == 1)
    }

    /// 直接引用的点位图片对当前 viewer 是否可见。
    async fn marker_image_authorized(
        &self,
        url: &str,
        viewer: Option<&Viewer<'_>>,
    ) -> Result<bool, MediaServiceError> {
        let direct = self
            .repo
            .markers_by_mark_image(url)
            .await
            .map_err(|error| db_error("查询点位图片直接引用", error))?;
        if direct.iter().any(|marker| can_view(marker, viewer)) {
            return Ok(true);
        }

        // 匿名在没有直接引用命中时不可见任何历史提案。
        let Some(viewer) = viewer else {
            return Ok(false);
        };
        // 已删除身份只能与匿名同权：不得凭旧 ADMIN/属主/提案作者身份读取私有提案图片。
        if viewer.deleted {
            return Ok(false);
        }

        let proposals = self
            .repo
            .proposals_with_marker_by_image_url(url)
            .await
            .map_err(|error| db_error("查询历史图片提案引用", error))?;
        for row in proposals {
            let (proposer_public_id, marker) = row.into_parts();
            // 关联点位由 INNER JOIN 保证仍存在；提案状态不参与判断。
            if viewer.role.eq_ignore_ascii_case("ADMIN") {
                return Ok(true);
            }
            if marker.deactivated {
                continue;
            }
            let Some(public_id) = viewer.public_id else {
                continue;
            };
            if marker.user_public_id.as_deref() == Some(public_id) {
                return Ok(true);
            }
            if proposer_public_id.as_deref() == Some(public_id) && can_view(&marker, Some(viewer)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// PG 提交后尽力失效点位缓存：500ms 超时 + 受控日志，错误不反转成功。
    async fn invalidate_marker_cache(&self) {
        match tokio::time::timeout(CACHE_INVALIDATE_TIMEOUT, self.cache.invalidate()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::warn!(
                target: "lycoris_backend::media",
                error_kind = ?error.kind(),
                "点位缓存失效失败，数据库写入已提交"
            ),
            Err(_) => tracing::warn!(
                target: "lycoris_backend::media",
                "点位缓存失效超时，数据库写入已提交"
            ),
        }
    }
}

/// 管理员判定：未删除且角色为 `ADMIN`（大小写不敏感）。二次验证由 HTTP 层负责。
fn require_admin(viewer: &Viewer<'_>) -> Result<(), MediaServiceError> {
    if !viewer.deleted && viewer.role.eq_ignore_ascii_case("ADMIN") {
        Ok(())
    } else {
        Err(MediaServiceError::Forbidden)
    }
}

/// 头像 URL 只接受合法的 `/uploads/avatars/<文件名>`；其他值按无头像处理。
fn sanitize_avatar_url(url: String) -> Option<String> {
    match crate::media::parse_media_url(&url) {
        Some((MediaDirectory::Avatars, _)) => Some(url),
        _ => None,
    }
}

/// 上传阶段的媒体错误映射：输入类错误为 400，繁忙为 503，其余为 500。
fn upload_error(error: MediaError) -> MediaServiceError {
    match error {
        MediaError::Busy => MediaServiceError::Unavailable,
        MediaError::NotFound => MediaServiceError::NotFound("图片不存在"),
        // 5 MiB 边界明确映射为 413；HTTP 用 `is_payload_too_large()` 分支，不靠字符串匹配。
        MediaError::TooLarge => MediaServiceError::PayloadTooLarge,
        // 编码失败属于服务端 500；图片格式/损坏等输入问题才是 400。
        MediaError::Root(_) | MediaError::Io(_) | MediaError::Task | MediaError::Encode => {
            MediaServiceError::Internal
        }
        other @ (MediaError::Empty
        | MediaError::Dimensions
        | MediaError::UnsupportedFormat
        | MediaError::Decode
        | MediaError::InvalidDirectory
        | MediaError::InvalidName
        | MediaError::InvalidPrefix
        | MediaError::InvalidConcurrency) => MediaServiceError::BadRequest(other.to_string()),
    }
}

/// 读取阶段的媒体错误映射：非法/不存在一律 404，I/O 与任务失败为 500。
fn read_error(error: MediaError) -> MediaServiceError {
    match error {
        MediaError::InvalidDirectory | MediaError::InvalidName | MediaError::NotFound => {
            MediaServiceError::NotFound("图片不存在")
        }
        MediaError::Io(_) | MediaError::Task => MediaServiceError::Internal,
        other => MediaServiceError::BadRequest(other.to_string()),
    }
}

fn db_error(context: &'static str, error: sqlx::Error) -> MediaServiceError {
    // 只记录受控的 SQLSTATE 与约束名，不输出底层完整错误、参数、图片 URL 或用户字段。
    let (code, constraint) = match error.as_database_error() {
        Some(database) => (
            database.code().map(|value| value.into_owned()),
            database.constraint().map(str::to_string),
        ),
        None => (None, None),
    };
    tracing::error!(
        target: "lycoris_backend::media",
        context,
        db_code = code.as_deref().unwrap_or("none"),
        db_constraint = constraint.as_deref().unwrap_or("none"),
        "媒体数据库操作失败"
    );
    if crate::db::is_timeout_sqlstate(&error) {
        MediaServiceError::Unavailable
    } else {
        MediaServiceError::Internal
    }
}
