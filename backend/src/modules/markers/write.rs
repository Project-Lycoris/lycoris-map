//! 阶段 3 点位写入、收藏、删除与审核事务核心。
//!
//! 本模块只做业务事务，不含 HTTP 路由/认证/`AppState`/config：身份由 [`Actor`] 显式传入，
//! 可见性复用读取层 `can_view`。固定 SQL 经 SQLx 编译期校验，参数全部绑定。
//!
//! 关键并发约束（与 `write-design.md` 一致）：
//! - 创建幂等只在 `client_request_id` 命中既有唯一约束时读回原点位，绝不把任意数据库错误
//!   当成幂等命中；
//! - 收藏先对点位取 `FOR KEY SHARE` 再插收藏，避免与删除点位交错产生孤立收藏；
//! - 停用在同一个事务内 `FOR UPDATE` 锁点位并推进版本，点位和全部关联数据留存；
//! - 编辑审核固定顺序：先 `FOR UPDATE` 锁提案、检查 `PENDING`、再 `FOR UPDATE` 锁点位、核对
//!   `base_marker_version` 与内容快照，然后更新中文原文/提案；译文仅由管理员手动同步；
//! - 原文与译文编辑共用同一 `marker.version`，任何文本变化都推进版本并以行锁串行；
//! - 提交成功后才使缓存失效，缓存故障只受控记录，绝不回退已提交的写入。

use sqlx::{PgConnection, PgPool};

use crate::modules::markers::cache::MarkerCache;
use crate::modules::markers::localization::{
    normalize, normalize_category_query, source_hash, translation_is_current,
};
use crate::modules::markers::model::{MarkerRow, TranslationRow, can_view};
use crate::modules::markers::write_model::{
    Actor, DEFAULT_VENUE_TYPE, EditProposalRow, MSG_MARK_IMAGE_UPLOAD_ONLY, MSG_MARKER_NOT_FOUND,
    MSG_PROPOSAL_ALREADY_HANDLED, MSG_PROPOSAL_NOT_FOUND, MSG_RELATED_MARKER_NOT_FOUND,
    MSG_STALE_VERSION, MSG_VENUE_TYPE_INVALID, MarkerCreateRequest, MarkerUpdateRequest,
    VENUE_TYPES, WriteError, db_len, utf16_len,
};

/// 数据库 `varchar` 列的字符上限（PostgreSQL `char_length` 语义）。
const TITLE_MAX: usize = 120;
const CATEGORY_MAX: usize = 64;
const USERNAME_MAX: usize = 64;
const PUBLIC_ID_MAX: usize = 64;
const CLIENT_REQUEST_ID_MAX: usize = 64;

/// 从 `map_markers` 一次写回的全部可变字段。
struct MarkerValues {
    category: String,
    title: String,
    description: Option<String>,
    source_language: String,
    is_public: bool,
    is_active: bool,
    open_time_start: Option<String>,
    open_time_end: Option<String>,
    venue_type: Option<String>,
    review_status: String,
    last_edited_by: Option<String>,
    last_edited_by_public_id: Option<String>,
    last_edited_by_owner: bool,
}

/// `resolve_edit_text` 的结果：目标语言与合并后的文本。
struct EditText {
    language: String,
    title: String,
    description: Option<String>,
}

/// `find_favorite_ids` 的标量行。
#[derive(Debug, sqlx::FromRow)]
struct FavoriteId {
    marker_id: i64,
}

/// 点位写入服务。
#[derive(Clone)]
pub struct MarkerWriteService {
    pool: PgPool,
    cache: MarkerCache,
}

impl MarkerWriteService {
    pub fn new(pool: PgPool, cache: MarkerCache) -> Self {
        Self { pool, cache }
    }

    /// `POST /api/markers`：创建点位（默认公开、`PENDING`），`(user_public_id, client_request_id)`
    /// 唯一约束处理并发重放；命中重放返回原点位且不再次使缓存失效。
    pub async fn create_marker(
        &self,
        actor: &Actor,
        _request_language: &str,
        req: MarkerCreateRequest,
    ) -> Result<MarkerRow, WriteError> {
        validate_actor(actor)?;
        // 与 Java 一致：先做最小必填字段检查（控制器层）与幂等键归一，再按 owner+key 读回已有点位；
        // 命中重放时不再校验坐标/类别/开放时段等载荷内容，也不改动原点位。
        if req.lat.is_none() || req.lng.is_none() || req.category.is_none() || req.title.is_none() {
            return Err(missing_fields());
        }
        let client_request_id = normalize_client_request_id(req.client_request_id.as_deref())?;
        if let Some(key) = client_request_id.as_deref()
            && let Some(existing) = self.find_by_client_request(&actor.public_id, key).await?
        {
            return Ok(existing);
        }

        // 首次创建：完整校验坐标/长度/开放时段。
        let lat = req.lat.expect("已校验必填字段");
        let lng = req.lng.expect("已校验必填字段");
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err(WriteError::BadRequest("lat/lng 不合法".to_string()));
        }
        if !lng.is_finite() || !(-180.0..=180.0).contains(&lng) {
            return Err(WriteError::BadRequest("lat/lng 不合法".to_string()));
        }
        let category = normalize_category(req.category.as_deref())?;
        let title = req.title.clone().expect("已校验必填字段");
        if db_len(&title) > TITLE_MAX {
            return Err(WriteError::BadRequest("title 过长".to_string()));
        }
        // 安全收紧：新建点位不得直接携带图片引用（null/空白归一为 null），
        // 图片只能经上传提案与审核流程关联，避免调用者伪造引用读取他人私有图片。
        let mark_image = normalize_mark_image(req.mark_image.as_deref())?;
        let (open_time_start, open_time_end) =
            resolve_open_window(req.open_time_start.as_deref(), req.open_time_end.as_deref())?;
        // Submission language is independent of the client UI; English is synced manually.
        let language = "zh";
        let is_public = req.is_public.unwrap_or(true);
        let is_active = req.is_active.unwrap_or(true);
        let venue_type = resolve_venue_type_create(&category, req.venue_type.as_deref())?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let inserted = sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/insert_marker.sql",
            lat,
            lng,
            category,
            title,
            req.description,
            language,
            is_public,
            is_active,
            open_time_start,
            open_time_end,
            mark_image,
            actor.username.as_str(),
            actor.public_id.as_str(),
            client_request_id.as_deref(),
            venue_type.as_deref(),
            actor.username.as_str(),
            actor.public_id.as_str(),
        )
        .fetch_one(&mut *tx)
        .await;

        match inserted {
            Ok(row) => {
                tx.commit().await.map_err(|error| log_db_error(&error))?;
                self.invalidate_after_commit().await;
                Ok(row)
            }
            Err(error) => {
                let conflict = client_request_conflict(&error) && client_request_id.is_some();
                drop(tx);
                if conflict {
                    // 唯一约束冲突：另一并发请求已插入，回读并返回同一 ID；其余错误一律内部错误。
                    let key = client_request_id.as_deref().unwrap_or_default();
                    self.find_by_client_request(&actor.public_id, key)
                        .await?
                        .ok_or(WriteError::Internal)
                } else {
                    Err(log_db_error(&error))
                }
            }
        }
    }

    /// `POST /api/markers/{id}/favorite`：锁点位 `FOR KEY SHARE` 验可见性后幂等插入收藏。
    pub async fn add_favorite(&self, actor: &Actor, marker_id: i64) -> Result<(), WriteError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let marker = lock_marker_key_share(&mut tx, marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        };
        if !can_view(&marker, Some(&actor.viewer())) {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        }
        sqlx::query_file!(
            "src/modules/markers/sql/insert_favorite.sql",
            actor.public_id.as_str(),
            marker_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(|error| log_db_error(&error))?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        Ok(())
    }

    /// `DELETE /api/markers/{id}/favorite`：不要求点位仍存在，删除不存在的收藏也成功。
    pub async fn remove_favorite(&self, actor: &Actor, marker_id: i64) -> Result<(), WriteError> {
        sqlx::query_file!(
            "src/modules/markers/sql/delete_favorite.sql",
            actor.public_id.as_str(),
            marker_id,
        )
        .execute(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))?;
        Ok(())
    }

    /// `DELETE /api/markers/{id}`：仅属主可停用；保留点位、收藏、译文和历史提案。
    pub async fn delete_owned_marker(
        &self,
        actor: &Actor,
        marker_id: i64,
    ) -> Result<(), WriteError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let marker = lock_marker(&mut tx, marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        };
        if marker.user_public_id.as_deref() != Some(actor.public_id.as_str()) {
            return Err(WriteError::Forbidden);
        }
        set_deactivated(&mut tx, marker_id, true).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(())
    }

    /// `PATCH /api/markers/{id}`：普通用户只创建 `PENDING` 编辑提案并返回未修改的点位。
    pub async fn create_edit_proposal(
        &self,
        actor: &Actor,
        _request_language: &str,
        marker_id: i64,
        req: MarkerUpdateRequest,
    ) -> Result<MarkerRow, WriteError> {
        validate_actor(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        // `FOR SHARE`：提案记录的 base 版本必须与用于文本基线的点位快照一致，
        // 并阻塞并发的原文/译文编辑直到本事务提交。
        let marker = lock_marker_share(&mut tx, marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        };
        if !can_view(&marker, Some(&actor.viewer())) {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        }

        let category = match req.category.as_deref() {
            Some(raw) => normalize_category(Some(raw))?,
            None => marker.category.clone(),
        };
        if db_len(&category) > CATEGORY_MAX {
            return Err(WriteError::BadRequest("category 过长".to_string()));
        }
        let text = resolve_edit_text(
            &mut tx,
            &marker,
            req.title.as_deref(),
            req.description.as_deref(),
            Some("zh"),
            "zh",
        )
        .await?;
        if db_len(&text.title) > TITLE_MAX {
            return Err(WriteError::BadRequest("title 过长".to_string()));
        }
        let (open_time_start, open_time_end) =
            if req.open_time_start.is_some() || req.open_time_end.is_some() {
                resolve_open_window(req.open_time_start.as_deref(), req.open_time_end.as_deref())?
            } else {
                (marker.open_time_start.clone(), marker.open_time_end.clone())
            };
        let is_public = req.is_public.unwrap_or(marker.is_public);
        let is_active = req.is_active.unwrap_or(marker.is_active);
        let venue_type = resolve_venue_type_update(
            &category,
            req.venue_type.as_deref(),
            marker.venue_type.as_deref(),
        )?;
        let is_owner = marker.user_public_id.as_deref() == Some(actor.public_id.as_str());
        if db_len(&text.language) > 2 {
            return Err(WriteError::BadRequest("language 不合法".to_string()));
        }

        let base_content = content_snapshot(&mut tx, &marker).await?;
        sqlx::query_file!(
            "src/modules/markers/sql/insert_edit_proposal.sql",
            marker.id,
            marker.title.as_str(),
            marker.lat,
            marker.lng,
            actor.username.as_str(),
            actor.public_id.as_str(),
            is_owner,
            category.as_str(),
            text.title.as_str(),
            text.description.as_deref(),
            "zh",
            is_public,
            is_active,
            open_time_start.as_deref(),
            open_time_end.as_deref(),
            venue_type.as_deref(),
            marker.version,
            base_content,
        )
        .execute(&mut *tx)
        .await
        .map_err(|error| log_db_error(&error))?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        // 提案不改动点位本身，公开缓存无需失效。
        Ok(marker)
    }

    /// `POST /api/admin/markers/{id}/approve`：管理员直接通过待审点位，推进版本。
    pub async fn approve_marker(
        &self,
        actor: &Actor,
        marker_id: i64,
    ) -> Result<MarkerRow, WriteError> {
        self.direct_review(actor, marker_id, "APPROVED").await
    }

    /// `POST /api/admin/markers/{id}/reject`：管理员直接驳回待审点位，推进版本。
    pub async fn reject_marker(
        &self,
        actor: &Actor,
        marker_id: i64,
    ) -> Result<MarkerRow, WriteError> {
        self.direct_review(actor, marker_id, "REJECTED").await
    }

    async fn direct_review(
        &self,
        actor: &Actor,
        marker_id: i64,
        status: &str,
    ) -> Result<MarkerRow, WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let marker = lock_marker(&mut tx, marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        };
        let mut values = base_values(&marker);
        values.review_status = status.to_string();
        let updated = write_marker_fields(&mut tx, marker_id, values).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(updated)
    }

    /// `GET /api/admin/markers/pending`：`PENDING` 点位，`updated_at DESC`。
    pub async fn pending_markers(&self, actor: &Actor) -> Result<Vec<MarkerRow>, WriteError> {
        ensure_admin(actor)?;
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/list_pending_markers.sql"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))
    }

    /// `GET /api/admin/markers/pending-edits`：`PENDING` 提案，`created_at DESC`。
    pub async fn pending_edit_proposals(
        &self,
        actor: &Actor,
    ) -> Result<Vec<EditProposalRow>, WriteError> {
        ensure_admin(actor)?;
        sqlx::query_file_as!(
            EditProposalRow,
            "src/modules/markers/sql/list_pending_edits.sql"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))
    }

    /// `POST /api/admin/markers/edit-proposals/{id}/approve`：同事务一次性地核对版本并落库。
    pub async fn approve_edit_proposal(
        &self,
        actor: &Actor,
        proposal_id: i64,
    ) -> Result<MarkerRow, WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        // 固定加锁顺序：先 proposal 后 marker；审核路径均如此，避免与其它路径交叉死锁。
        let proposal = lock_edit_proposal(&mut tx, proposal_id).await?;
        let Some(proposal) = proposal else {
            return Err(WriteError::NotFound(MSG_PROPOSAL_NOT_FOUND.to_string()));
        };
        if !proposal.status.eq_ignore_ascii_case("PENDING") {
            return Err(WriteError::BadRequest(
                MSG_PROPOSAL_ALREADY_HANDLED.to_string(),
            ));
        }
        let marker = lock_marker(&mut tx, proposal.marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(
                MSG_RELATED_MARKER_NOT_FOUND.to_string(),
            ));
        };
        if marker.deactivated {
            return Err(WriteError::NotFound(
                MSG_RELATED_MARKER_NOT_FOUND.to_string(),
            ));
        }
        if proposal.base_marker_version.is_none()
            || (proposal.base_marker_version != Some(marker.version)
                && proposal.base_content.as_ref()
                    != Some(&content_snapshot(&mut tx, &marker).await?))
        {
            return Err(WriteError::Conflict(MSG_STALE_VERSION.to_string()));
        }

        // Public contributions always update Chinese source content. Only the
        // explicit admin edit path below can write a manual English translation.
        let mut values = base_values(&marker);
        values.source_language = "zh".to_string();
        values.category = normalize_category(Some(&proposal.category))?;
        values.is_public = proposal.is_public;
        values.is_active = proposal.is_active;
        values.open_time_start = proposal.open_time_start.clone();
        values.open_time_end = proposal.open_time_end.clone();
        values.venue_type = proposal.venue_type.clone();
        values.review_status = "APPROVED".to_string();
        values.last_edited_by = Some(proposal.proposer_username.clone());
        values.last_edited_by_public_id = proposal.proposer_public_id.clone();
        values.last_edited_by_owner = proposal.proposer_is_owner;
        values.title = proposal.title.clone();
        values.description = proposal.description.clone();
        let updated = write_marker_fields(&mut tx, marker.id, values).await?;
        update_proposal_status(&mut tx, proposal_id, "APPROVED", &actor.username).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(updated)
    }

    /// `POST /api/admin/markers/edit-proposals/{id}/reject`：一次性把 `PENDING` 提案置驳回。
    pub async fn reject_edit_proposal(
        &self,
        actor: &Actor,
        proposal_id: i64,
    ) -> Result<(), WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let proposal = lock_edit_proposal(&mut tx, proposal_id).await?;
        let Some(proposal) = proposal else {
            return Err(WriteError::NotFound(MSG_PROPOSAL_NOT_FOUND.to_string()));
        };
        if !proposal.status.eq_ignore_ascii_case("PENDING") {
            return Err(WriteError::BadRequest(
                MSG_PROPOSAL_ALREADY_HANDLED.to_string(),
            ));
        }
        update_proposal_status(&mut tx, proposal_id, "REJECTED", &actor.username).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        Ok(())
    }

    /// `PATCH /api/admin/markers/{id}`：管理员直接编辑（不生成提案），推进版本并置 `APPROVED`。
    pub async fn admin_update_marker(
        &self,
        actor: &Actor,
        request_language: &str,
        marker_id: i64,
        req: MarkerUpdateRequest,
    ) -> Result<MarkerRow, WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let marker = lock_marker(&mut tx, marker_id).await?;
        let Some(marker) = marker else {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        };
        let category = match req.category.as_deref() {
            Some(raw) => normalize_category(Some(raw))?,
            None => marker.category.clone(),
        };
        let text = resolve_edit_text(
            &mut tx,
            &marker,
            req.title.as_deref(),
            req.description.as_deref(),
            req.language.as_deref(),
            request_language,
        )
        .await?;
        let target = normalize(Some(&text.language)).to_string();
        let source_edit = target == normalize(Some(&marker.source_language));
        let (open_time_start, open_time_end) =
            if req.open_time_start.is_some() || req.open_time_end.is_some() {
                resolve_open_window(req.open_time_start.as_deref(), req.open_time_end.as_deref())?
            } else {
                (marker.open_time_start.clone(), marker.open_time_end.clone())
            };

        let venue_type = resolve_venue_type_update(
            &category,
            req.venue_type.as_deref(),
            marker.venue_type.as_deref(),
        )?;
        let mut values = base_values(&marker);
        values.category = category;
        values.is_public = req.is_public.unwrap_or(marker.is_public);
        values.is_active = req.is_active.unwrap_or(marker.is_active);
        values.open_time_start = open_time_start;
        values.open_time_end = open_time_end;
        values.venue_type = venue_type;
        values.review_status = "APPROVED".to_string();
        if source_edit {
            values.title = text.title.clone();
            values.description = text.description.clone();
        }
        let updated = write_marker_fields(&mut tx, marker.id, values).await?;
        if !source_edit {
            upsert_translation(
                &mut tx,
                updated.id,
                &target,
                &text.title,
                text.description.as_deref(),
                &source_hash(&updated),
            )
            .await?;
        }
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(updated)
    }

    /// `DELETE /api/admin/markers/{id}`：管理员停用；保留全部数据并使公开缓存失效。
    pub async fn admin_delete_marker(
        &self,
        actor: &Actor,
        marker_id: i64,
    ) -> Result<(), WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        let marker = lock_marker(&mut tx, marker_id).await?;
        if marker.is_none() {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        }
        set_deactivated(&mut tx, marker_id, true).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(())
    }

    /// Restore only the deactivation flag; preserve the previous visibility/review state.
    pub async fn admin_restore_marker(
        &self,
        actor: &Actor,
        marker_id: i64,
    ) -> Result<(), WriteError> {
        ensure_admin(actor)?;
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| log_db_error(&error))?;
        if lock_marker(&mut tx, marker_id).await?.is_none() {
            return Err(WriteError::NotFound(MSG_MARKER_NOT_FOUND.to_string()));
        }
        set_deactivated(&mut tx, marker_id, false).await?;
        tx.commit().await.map_err(|error| log_db_error(&error))?;
        self.invalidate_after_commit().await;
        Ok(())
    }

    /// `GET /api/markers/all` 与 `/api/admin/markers/all`：管理员全部点位读取。
    pub async fn list_all_markers(&self, actor: &Actor) -> Result<Vec<MarkerRow>, WriteError> {
        ensure_admin(actor)?;
        sqlx::query_file_as!(MarkerRow, "src/modules/markers/sql/find_all.sql")
            .fetch_all(&self.pool)
            .await
            .map_err(|error| log_db_error(&error))
    }

    /// `GET /api/markers/me/created`：本人创建的点位（含私有/待审，不做可见性过滤）。
    pub async fn list_created(&self, actor: &Actor) -> Result<Vec<MarkerRow>, WriteError> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_by_user_public_id.sql",
            actor.public_id.as_str(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))
    }

    /// `GET /api/markers/me/favorites`：可见收藏点位 ID（按收藏顺序）。
    pub async fn favorite_ids(&self, actor: &Actor) -> Result<Vec<i64>, WriteError> {
        Ok(self
            .visible_favorites(actor)
            .await?
            .into_iter()
            .map(|marker| marker.id)
            .collect())
    }

    /// `GET /api/markers/me/favorites/details`：可见收藏点位完整行（按收藏顺序）。
    pub async fn favorite_markers(&self, actor: &Actor) -> Result<Vec<MarkerRow>, WriteError> {
        self.visible_favorites(actor).await
    }

    async fn visible_favorites(&self, actor: &Actor) -> Result<Vec<MarkerRow>, WriteError> {
        let favorites = sqlx::query_file_as!(
            FavoriteId,
            "src/modules/markers/sql/find_favorite_ids.sql",
            actor.public_id.as_str(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))?;
        if favorites.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = favorites
            .iter()
            .map(|favorite| favorite.marker_id)
            .collect();
        let markers = sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_by_ids_any_visibility.sql",
            &ids,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))?;
        let mut by_id: std::collections::HashMap<i64, MarkerRow> = markers
            .into_iter()
            .map(|marker| (marker.id, marker))
            .collect();
        let viewer = actor.viewer();
        Ok(ids
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .filter(|marker| can_view(marker, Some(&viewer)))
            .collect())
    }

    async fn find_by_client_request(
        &self,
        public_id: &str,
        client_request_id: &str,
    ) -> Result<Option<MarkerRow>, WriteError> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_by_client_request.sql",
            public_id,
            client_request_id,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| log_db_error(&error))
    }

    /// 提交成功后的缓存失效；任何失败只做受控日志，不改变已提交的业务结果。
    async fn invalidate_after_commit(&self) {
        if let Err(error) = self.cache.invalidate().await {
            tracing::warn!(
                target: "lycoris_backend::markers::write",
                kind = ?error.kind(),
                "点位缓存失效失败，写入已提交"
            );
        }
    }
}

fn validate_actor(actor: &Actor) -> Result<(), WriteError> {
    if db_len(&actor.username) > USERNAME_MAX {
        return Err(WriteError::BadRequest("用户名过长".to_string()));
    }
    if db_len(&actor.public_id) > PUBLIC_ID_MAX {
        return Err(WriteError::BadRequest("用户标识过长".to_string()));
    }
    Ok(())
}

fn ensure_admin(actor: &Actor) -> Result<(), WriteError> {
    if actor.is_admin {
        Ok(())
    } else {
        Err(WriteError::Forbidden)
    }
}

fn missing_fields() -> WriteError {
    WriteError::BadRequest("缺少必要字段".to_string())
}

fn normalize_category(raw: Option<&str>) -> Result<String, WriteError> {
    let Some(raw) = raw else {
        return Err(WriteError::BadRequest("category 不能为空".to_string()));
    };
    match normalize_category_query(raw) {
        Ok(value) => Ok(value),
        Err(crate::error::ApiError::BadRequest(message)) => Err(WriteError::BadRequest(message)),
        Err(_) => Err(WriteError::BadRequest("不支持的 category".to_string())),
    }
}

/// 归一显式传入的场所标签：`None`/空白视为未提供，返回 `Ok(None)`；非空白必须落在白名单，
/// 否则 400。
fn normalize_venue_type(raw: Option<&str>) -> Result<Option<&str>, WriteError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if VENUE_TYPES.contains(&trimmed) {
        Ok(Some(trimmed))
    } else {
        Err(WriteError::BadRequest(MSG_VENUE_TYPE_INVALID.to_string()))
    }
}

/// 新建：`accessible_toilet` 未传值默认 `other`；其它类别只允许 NULL，传非空值 400。
fn resolve_venue_type_create(
    category: &str,
    raw: Option<&str>,
) -> Result<Option<String>, WriteError> {
    let explicit = normalize_venue_type(raw)?;
    if category != "accessible_toilet" {
        if explicit.is_some() {
            return Err(WriteError::BadRequest(MSG_VENUE_TYPE_INVALID.to_string()));
        }
        return Ok(None);
    }
    Ok(Some(explicit.unwrap_or(DEFAULT_VENUE_TYPE).to_string()))
}

/// 更新：未传值保留原值（转入 `accessible_toilet` 且原值为空时默认 `other`），
/// 转出 `accessible_toilet` 时清除；传非空值必须是 `accessible_toilet` 且在白名单内。
fn resolve_venue_type_update(
    category: &str,
    raw: Option<&str>,
    existing: Option<&str>,
) -> Result<Option<String>, WriteError> {
    let explicit = normalize_venue_type(raw)?;
    if category != "accessible_toilet" {
        if explicit.is_some() {
            return Err(WriteError::BadRequest(MSG_VENUE_TYPE_INVALID.to_string()));
        }
        return Ok(None);
    }
    if let Some(value) = explicit {
        return Ok(Some(value.to_string()));
    }
    Ok(Some(existing.unwrap_or(DEFAULT_VENUE_TYPE).to_string()))
}

/// 新建点位 `markImage` 归一：`None` 与空白串为 `None`，任何非空值一律 400。
fn normalize_mark_image(raw: Option<&str>) -> Result<Option<String>, WriteError> {
    match raw {
        None => Ok(None),
        Some(value) if value.trim().is_empty() => Ok(None),
        Some(_) => Err(WriteError::BadRequest(
            MSG_MARK_IMAGE_UPLOAD_ONLY.to_string(),
        )),
    }
}

fn resolve_language(explicit: Option<&str>, request_language: &str) -> String {
    match explicit {
        Some(value) => normalize(Some(value)).to_string(),
        None => normalize(Some(request_language)).to_string(),
    }
}

fn normalize_client_request_id(raw: Option<&str>) -> Result<Option<String>, WriteError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if utf16_len(trimmed) > CLIENT_REQUEST_ID_MAX {
        return Err(WriteError::BadRequest("clientRequestId 过长".to_string()));
    }
    Ok(Some(trimmed.to_string()))
}

/// 归一开放时间窗口；只填一边时报契约错误。空串视为未填。
fn resolve_open_window(
    start: Option<&str>,
    end: Option<&str>,
) -> Result<(Option<String>, Option<String>), WriteError> {
    let start = match start {
        Some(value) => normalize_open_time(value)?,
        None => None,
    };
    let end = match end {
        Some(value) => normalize_open_time(value)?,
        None => None,
    };
    if start.is_some() != end.is_some() {
        return Err(WriteError::BadRequest(
            "请同时填写开始和结束时间，或都留空".to_string(),
        ));
    }
    Ok((start, end))
}

/// 与 Java `MapMarkerService.normalizeOpenTime` 一致：解析 `HH:mm[:ss[.fff]]` 并输出 `HH:mm`。
fn normalize_open_time(raw: &str) -> Result<Option<String>, WriteError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let (hour, minute) = parse_local_time(trimmed)
        .ok_or_else(|| WriteError::BadRequest("时间格式不合法，请使用 HH:mm".to_string()))?;
    Ok(Some(format!("{hour:02}:{minute:02}")))
}

fn parse_local_time(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.split(':');
    let hour = parts.next()?;
    let minute = parts.next()?;
    let rest = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let (second, fraction) = match rest {
        None => (None, None),
        Some(rest) => {
            let mut split = rest.splitn(2, '.');
            let second = split.next().unwrap_or_default();
            (Some(second), split.next())
        }
    };
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    if let Some(second) = second
        && second.len() != 2
    {
        return None;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    if !hour.bytes().all(|byte| byte.is_ascii_digit())
        || !minute.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    if let Some(second) = second {
        if !second.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if second.parse::<u32>().ok()? > 59 {
            return None;
        }
    }
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

/// Java `resolveEditText` 的等价实现：只在文本字段出现时才协商目标语言。
async fn resolve_edit_text(
    conn: &mut PgConnection,
    marker: &MarkerRow,
    title: Option<&str>,
    description: Option<&str>,
    language: Option<&str>,
    request_language: &str,
) -> Result<EditText, WriteError> {
    let source_language = normalize(Some(&marker.source_language)).to_string();
    if title.is_none() && description.is_none() {
        // 没有任何文本字段：语言取原文语言，文本保持原文，绝不因请求语言切换语言。
        return Ok(EditText {
            language: source_language,
            title: marker.title.clone(),
            description: marker.description.clone(),
        });
    }
    let target = resolve_language(language, request_language);
    let mut baseline_title = marker.title.clone();
    let mut baseline_description = marker.description.clone();
    if target != source_language {
        let translation = sqlx::query_file_as!(
            TranslationRow,
            "src/modules/markers/sql/find_translation_for_language.sql",
            marker.id,
            target.as_str(),
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(|error| log_db_error(&error))?;
        match translation {
            Some(translation) if translation_is_current(marker, &translation) => {
                baseline_title = translation.title;
                baseline_description = translation.description;
            }
            _ => {
                if title.is_none() || description.is_none() {
                    return Err(WriteError::BadRequest(
                        "该语言尚无有效译文，请同时填写标题和描述（描述可为空）".to_string(),
                    ));
                }
            }
        }
    }
    Ok(EditText {
        language: target,
        title: title.map(str::to_string).unwrap_or(baseline_title),
        description: description.map(str::to_string).or(baseline_description),
    })
}

/// Fields a content proposal can overwrite (plus location/visibility guards).
/// Images, review status and audit timestamps cannot make an otherwise current edit stale.
async fn content_snapshot(
    conn: &mut PgConnection,
    marker: &MarkerRow,
) -> Result<serde_json::Value, WriteError> {
    let last_deactivated_version = sqlx::query_scalar!(
        "SELECT last_deactivated_version FROM map_markers WHERE id = $1",
        marker.id
    )
    .fetch_one(conn)
    .await
    .map_err(|error| log_db_error(&error))?;
    Ok(serde_json::json!({
        "category": marker.category, "title": marker.title, "description": marker.description,
        "source_language": marker.source_language,
        "lat": format!("{:016x}", marker.lat.to_bits()), "lng": format!("{:016x}", marker.lng.to_bits()),
        "is_public": marker.is_public, "is_active": marker.is_active,
        "open_time_start": marker.open_time_start, "open_time_end": marker.open_time_end,
        "venue_type": marker.venue_type, "deactivated": marker.deactivated,
        "last_deactivated_version": last_deactivated_version,
    }))
}

fn base_values(marker: &MarkerRow) -> MarkerValues {
    MarkerValues {
        category: marker.category.clone(),
        title: marker.title.clone(),
        description: marker.description.clone(),
        source_language: marker.source_language.clone(),
        is_public: marker.is_public,
        is_active: marker.is_active,
        open_time_start: marker.open_time_start.clone(),
        open_time_end: marker.open_time_end.clone(),
        venue_type: marker.venue_type.clone(),
        review_status: marker.review_status.clone(),
        last_edited_by: marker.last_edited_by.clone(),
        last_edited_by_public_id: marker.last_edited_by_public_id.clone(),
        last_edited_by_owner: marker.last_edited_by_owner,
    }
}

async fn lock_marker(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
) -> Result<Option<MarkerRow>, WriteError> {
    sqlx::query_file_as!(
        MarkerRow,
        "src/modules/markers/sql/lock_marker.sql",
        marker_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))
}

async fn lock_marker_share(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
) -> Result<Option<MarkerRow>, WriteError> {
    sqlx::query_file_as!(
        MarkerRow,
        "src/modules/markers/sql/lock_marker_share.sql",
        marker_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))
}

async fn lock_marker_key_share(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
) -> Result<Option<MarkerRow>, WriteError> {
    sqlx::query_file_as!(
        MarkerRow,
        "src/modules/markers/sql/lock_marker_key_share.sql",
        marker_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))
}

async fn lock_edit_proposal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    proposal_id: i64,
) -> Result<Option<EditProposalRow>, WriteError> {
    sqlx::query_file_as!(
        EditProposalRow,
        "src/modules/markers/sql/lock_edit_proposal.sql",
        proposal_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))
}

async fn write_marker_fields(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
    values: MarkerValues,
) -> Result<MarkerRow, WriteError> {
    let MarkerValues {
        category,
        title,
        description,
        source_language,
        is_public,
        is_active,
        open_time_start,
        open_time_end,
        venue_type,
        review_status,
        last_edited_by,
        last_edited_by_public_id,
        last_edited_by_owner,
    } = values;
    sqlx::query_file_as!(
        MarkerRow,
        "src/modules/markers/sql/update_marker_fields.sql",
        marker_id,
        category,
        title,
        description,
        source_language,
        is_public,
        is_active,
        open_time_start,
        open_time_end,
        review_status,
        last_edited_by,
        last_edited_by_public_id,
        last_edited_by_owner,
        venue_type,
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))
}

async fn upsert_translation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
    language: &str,
    title: &str,
    description: Option<&str>,
    source_hash: &str,
) -> Result<(), WriteError> {
    sqlx::query_file!(
        "src/modules/markers/sql/upsert_translation_manual.sql",
        marker_id,
        language,
        title,
        description,
        source_hash,
    )
    .execute(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))?;
    Ok(())
}

async fn update_proposal_status(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    proposal_id: i64,
    status: &str,
    reviewer: &str,
) -> Result<(), WriteError> {
    sqlx::query_file!(
        "src/modules/markers/sql/update_proposal_status.sql",
        proposal_id,
        status,
        reviewer,
        chrono::Utc::now(),
    )
    .execute(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))?;
    Ok(())
}

async fn set_deactivated(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    marker_id: i64,
    deactivated: bool,
) -> Result<(), WriteError> {
    sqlx::query_file!(
        "src/modules/markers/sql/set_deactivated.sql",
        marker_id,
        deactivated
    )
    .execute(&mut **tx)
    .await
    .map_err(|error| log_db_error(&error))?;
    Ok(())
}

/// 判断错误是否为 `(user_public_id, client_request_id)` 唯一约束冲突。
fn client_request_conflict(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(database) => {
            database.code().as_deref() == Some("23505")
                && database.constraint() == Some("uk_map_markers_user_client_request")
        }
        _ => false,
    }
}

/// 受控数据库错误日志：只记录错误码/约束名，不携带 SQL、参数或用户资料。
///
/// 语句取消（57014）与锁等待超时（55P03）是可重试的依赖不可用，受控映射 503；未知错误保持 500。
fn log_db_error(error: &sqlx::Error) -> WriteError {
    match error {
        sqlx::Error::Database(database) => tracing::error!(
            target: "lycoris_backend::markers::write",
            code = database.code().as_deref(),
            constraint = database.constraint(),
            "点位写入数据库错误"
        ),
        _ => tracing::error!(
            target: "lycoris_backend::markers::write",
            "点位写入数据库错误"
        ),
    }
    if crate::db::is_timeout_sqlstate(error) {
        WriteError::Unavailable
    } else {
        WriteError::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_client_request_id, normalize_open_time, resolve_open_window};

    #[test]
    fn open_time_matches_java_local_time_rules() {
        assert_eq!(
            normalize_open_time("08:00").unwrap().as_deref(),
            Some("08:00")
        );
        assert_eq!(
            normalize_open_time(" 23:59:59.123 ").unwrap().as_deref(),
            Some("23:59")
        );
        assert_eq!(normalize_open_time("").unwrap(), None);
        assert_eq!(normalize_open_time("   ").unwrap(), None);
        assert!(normalize_open_time("8:00").is_err(), "Java 要求两位小时");
        assert!(normalize_open_time("24:00").is_err());
        assert!(normalize_open_time("08:60").is_err());
        assert!(normalize_open_time("08:00:60").is_err());
        assert!(normalize_open_time("08:00:00.").is_err());
        assert!(normalize_open_time("08:00:00.x").is_err());
        assert!(normalize_open_time("abc").is_err());
    }

    #[test]
    fn open_window_requires_both_sides() {
        assert_eq!(
            resolve_open_window(Some("08:00"), Some("20:00")).unwrap(),
            (Some("08:00".to_string()), Some("20:00".to_string()))
        );
        assert_eq!(
            resolve_open_window(Some(""), None).unwrap(),
            (None, None),
            "空串视为未填，两边都为 null 合法"
        );
        assert!(resolve_open_window(Some("08:00"), None).is_err());
        assert!(resolve_open_window(None, Some("20:00")).is_err());
    }

    #[test]
    fn client_request_id_trims_and_bounds() {
        assert_eq!(normalize_client_request_id(None).unwrap(), None);
        assert_eq!(normalize_client_request_id(Some("   ")).unwrap(), None);
        assert_eq!(
            normalize_client_request_id(Some("  abc  "))
                .unwrap()
                .as_deref(),
            Some("abc")
        );
        assert!(normalize_client_request_id(Some(&"a".repeat(65))).is_err());
        assert!(normalize_client_request_id(Some(&"a".repeat(64))).is_ok());
    }
}
