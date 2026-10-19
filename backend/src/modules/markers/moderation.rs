//! 审核事件、审核理由与提交者统计。
//!
//! 事件写入只接收调用方已开启的事务：审核/管理业务写入与事件必须在同一事务中提交，
//! 业务失败回滚时不会留下孤立事件。理由文本按字符计数，trim 后为空视为未填写。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};

/// 拒绝理由的最大字符数（PostgreSQL `varchar(500)` 语义）。
pub const REVIEW_NOTE_MAX_CHARS: usize = 500;

/// 提交者历史提交计数（新增点位 + 文字提案 + 图片提案）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct ProposerSubmissionStats {
    pub approved: i64,
    pub rejected: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProposerStatsRow {
    pub public_id: Option<String>,
    pub approved: Option<i64>,
    pub rejected: Option<i64>,
    pub pending: Option<i64>,
}

/// 审核事件响应行。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ModerationEventDto {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub actor_public_id: String,
    pub actor_username: String,
    pub action: String,
    pub target_kind: String,
    pub target_id: i64,
    pub marker_id: Option<i64>,
    pub marker_title: Option<String>,
    pub target_label: Option<String>,
    pub note: Option<String>,
}

/// `/api/markers/me/submissions` 的单条响应。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDto {
    pub kind: String,
    pub id: i64,
    pub marker_id: i64,
    pub marker_title: String,
    pub status: String,
    pub review_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

/// 归一拒绝理由：trim；空白视为未填写；按字符计上限 500。
pub fn normalize_review_note(raw: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(trimmed) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if trimmed.chars().count() > REVIEW_NOTE_MAX_CHARS {
        return Err("理由过长");
    }
    Ok(Some(trimmed.to_string()))
}

/// 在同一事务中写入一条审核事件。
#[allow(clippy::too_many_arguments)]
pub async fn record_event(
    tx: &mut Transaction<'_, Postgres>,
    actor_public_id: &str,
    actor_username: &str,
    action: &str,
    target_kind: &str,
    target_id: i64,
    marker_id: Option<i64>,
    note: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query_file!(
        "src/modules/markers/sql/insert_moderation_event.sql",
        actor_public_id,
        actor_username,
        action,
        target_kind,
        target_id,
        marker_id,
        note,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 批量读取提交者历史计数；没有提交记录的用户不出现在结果中。
pub async fn proposer_stats(
    pool: &PgPool,
    public_ids: &[String],
) -> Result<HashMap<String, ProposerSubmissionStats>, sqlx::Error> {
    if public_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_file_as!(
        ProposerStatsRow,
        "src/modules/markers/sql/proposer_submission_stats.sql",
        public_ids,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.public_id.map(|public_id| {
                (
                    public_id,
                    ProposerSubmissionStats {
                        approved: row.approved.unwrap_or(0),
                        rejected: row.rejected.unwrap_or(0),
                        pending: row.pending.unwrap_or(0),
                    },
                )
            })
        })
        .collect())
}

/// 模糊搜索转义：`%`、`_`、`\` 均按字面量匹配（配合 SQL `ESCAPE '\'`）。
pub fn escape_like(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{escape_like, normalize_review_note};

    #[test]
    fn review_note_trims_bounds_and_treats_blank_as_empty() {
        assert_eq!(normalize_review_note(None).unwrap(), None);
        assert_eq!(normalize_review_note(Some("   ")).unwrap(), None);
        assert_eq!(
            normalize_review_note(Some("  理由  ")).unwrap().as_deref(),
            Some("理由")
        );
        assert!(normalize_review_note(Some(&"字".repeat(500))).is_ok());
        assert_eq!(
            normalize_review_note(Some(&"字".repeat(501))).unwrap_err(),
            "理由过长"
        );
    }

    #[test]
    fn like_escape_escapes_wildcards_and_backslash() {
        assert_eq!(escape_like(r"100%_\x"), r"100\%\_\\x");
        assert_eq!(escape_like("普通"), "普通");
    }
}
