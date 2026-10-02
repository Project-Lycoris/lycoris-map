//! 本地化与读取期纯函数。
//!
//! 严格对照 Java `MarkerLanguage` / `MarkerSourceHash` / `MapMarkerService` 的读取行为：
//! - 语言协商优先级：显式 `lang` > `Accept-Language`（存在且非空白）> `X-App-Language`；
//!   非空 `Accept-Language` 不受支持或非法时直接 `zh`，**不回退** `X-App-Language`；
//! - 源文本哈希：`[规范化语言, title 或 "", description 或 ""]` 紧凑 UTF-8 JSON 的 SHA-256，
//!   非 ASCII 不转义、控制字符小写 `\uXXXX`；
//! - 类别读取归一与查询白名单、开放时间实时计算。

use std::fmt::Write as _;

use axum::http::{HeaderMap, HeaderName, header};
use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::modules::markers::model::{MarkerRow, TranslationRow};

/// 受支持类别（查询白名单）。
pub const SUPPORTED_CATEGORIES: [&str; 4] = [
    "accessible_toilet",
    "friendly_clinic",
    "baby_room",
    "self_definition",
];

/// 历史类别，读/写/查询一律归一为 `self_definition`。
const LEGACY_TO_SELF_DEFINITION: [&str; 2] = ["safe_place", "dangerous_place"];

const X_APP_LANGUAGE: HeaderName = HeaderName::from_static("x-app-language");

/// 归一为 `en` / `zh`。`en` 或 `en-` 前缀为 `en`，`zh` 或 `zh-` 前缀为 `zh`，其余 `zh`。
pub fn normalize(value: Option<&str>) -> &'static str {
    let Some(value) = value else {
        return "zh";
    };
    let lower = value.trim().to_lowercase().replace('_', "-");
    if lower == "en" || lower.starts_with("en-") {
        "en"
    } else {
        // `zh` / `zh-` 前缀与任何不受支持或空值都落到中文。
        "zh"
    }
}

/// 读取语言：显式查询参数（只要存在就采用）优先，否则走请求头。
pub fn for_read(explicit: Option<&str>, headers: &HeaderMap) -> &'static str {
    if let Some(value) = explicit {
        return normalize(Some(value));
    }
    from_headers(headers)
}

/// 请求头语言：`Accept-Language` 存在且非空白时按其解析；否则 `X-App-Language`。
pub fn from_headers(headers: &HeaderMap) -> &'static str {
    if let Some(accept) = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        && !accept.trim().is_empty()
    {
        return parse_accept_language(accept);
    }
    normalize(
        headers
            .get(X_APP_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
    )
}

/// 解析 `Accept-Language`。任何解析错误或全部不受支持时返回 `zh`。
fn parse_accept_language(raw: &str) -> &'static str {
    let Ok(ranges) = parse_ranges(raw) else {
        return "zh";
    };
    for (range, weight) in ranges {
        if weight > 0.0
            && let Some(language) = supported_range(&range)
        {
            return language;
        }
    }
    "zh"
}

/// 按 Java `Locale.LanguageRange.parse` 的实际规则解析。
///
/// - 以 `,` 分隔，尾部空项丢弃；其余空项（含首项）视为非法；
/// - 每项的 `; ... q ... = <weight>` 后缀被识别为权重（`q` 大小写不敏感、允许空白）；
///   不匹配该后缀的 `;` 会使整项被当作语言范围而非法；
/// - 语言范围必须为 `*` 或 `[A-Za-z]{1,8}(-[A-Za-z0-9]{1,8})*`；
/// - 权重按 `Double.parseDouble` 语义解析且必须落在 `[0,1]`；
/// - 重复语言范围仅保留首次出现的权重；
/// - 最终按权重降序稳定排序。
fn parse_ranges(raw: &str) -> Result<Vec<(String, f64)>, ()> {
    let mut items: Vec<&str> = raw.split(',').collect();
    while items.last().is_some_and(|item| item.is_empty()) {
        items.pop();
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let mut ranges: Vec<(String, f64)> = Vec::new();
    for item in items {
        if item.trim().is_empty() {
            return Err(());
        }
        let (range_part, weight) = match split_weight(item) {
            Some((range_part, value)) => {
                let parsed: f64 = value.parse().map_err(|_| ())?;
                if !(0.0..=1.0).contains(&parsed) {
                    return Err(());
                }
                (range_part, parsed)
            }
            None => (item.trim(), 1.0),
        };
        let range = range_part.trim().to_lowercase();
        if !valid_range(&range) {
            return Err(());
        }
        // 重复语言范围以首次出现的权重为准，后续重复项不再覆盖。
        if !ranges.iter().any(|(existing, _)| existing == &range) {
            ranges.push((range, weight));
        }
    }

    ranges.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(ranges)
}

/// 识别 `; <空白> q <空白> = <值>` 后缀；返回 `(范围部分, 权重文本)`。
fn split_weight(item: &str) -> Option<(&str, &str)> {
    let bytes = item.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b';' {
            let mut cursor = index + 1;
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && (bytes[cursor] == b'q' || bytes[cursor] == b'Q') {
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
                if cursor < bytes.len() && bytes[cursor] == b'=' {
                    return Some((&item[..index], item[cursor + 1..].trim()));
                }
            }
        }
        index += 1;
    }
    None
}

/// 与 Java `LanguageRange` 一致的语法校验：首子标签纯字母，后续子标签字母数字，长度 1..=8。
fn valid_range(range: &str) -> bool {
    if range == "*" {
        return true;
    }
    let mut subtags = range.split('-');
    let Some(first) = subtags.next() else {
        return false;
    };
    valid_subtag(first, true) && subtags.all(|subtag| valid_subtag(subtag, false))
}

fn valid_subtag(subtag: &str, first: bool) -> bool {
    !subtag.is_empty()
        && subtag.len() <= 8
        && subtag.chars().all(|character| {
            if first {
                character.is_ascii_alphabetic()
            } else {
                character.is_ascii_alphanumeric()
            }
        })
}

fn supported_range(range: &str) -> Option<&'static str> {
    if range == "en" || range.starts_with("en-") {
        Some("en")
    } else if range == "zh" || range.starts_with("zh-") {
        Some("zh")
    } else {
        None
    }
}

/// 源文本哈希（SHA-256，十六进制小写）。
pub fn source_hash(row: &MarkerRow) -> String {
    source_hash_components(
        &row.source_language,
        Some(&row.title),
        row.description.as_deref(),
    )
}

/// 显式构件的源文本哈希，供写入阶段与测试使用。
pub fn source_hash_components(
    source_language: &str,
    title: Option<&str>,
    description: Option<&str>,
) -> String {
    hash_parts(source_language, title, description)
}

fn hash_parts(source_language: &str, title: Option<&str>, description: Option<&str>) -> String {
    let language = normalize(Some(source_language));
    // 与 Java `MarkerSourceHash` 一致：三元素字符串数组的紧凑 UTF-8 JSON，
    // null 文本为空串，非 ASCII 不转义，控制字符小写 `\uXXXX`。交给 serde_json 标准编码，
    // 不再手写 JSON serializer。
    let json =
        serde_json::json!([language, title.unwrap_or(""), description.unwrap_or("")]).to_string();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    to_hex(&hasher.finalize())
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// 译文是否对当前原文有效：属于同一点位、语言为 en/zh、与原文语言不同、且源哈希一致。
pub fn translation_is_current(row: &MarkerRow, translation: &TranslationRow) -> bool {
    row.id == translation.marker_id
        && (translation.language == "en" || translation.language == "zh")
        && normalize(Some(&row.source_language)) != translation.language
        && source_hash(row) == translation.source_hash
}

/// 返回 `(title, description, contentLanguage)`；目标译文缺失或失效时回退原文。
pub fn localize_text(
    row: &MarkerRow,
    translation: Option<&TranslationRow>,
    target: &'static str,
) -> (String, Option<String>, &'static str) {
    let source_language = normalize(Some(&row.source_language));
    if let Some(translation) = translation
        && translation.language == target
        && translation_is_current(row, translation)
    {
        return (
            translation.title.clone(),
            translation.description.clone(),
            target,
        );
    }
    (row.title.clone(), row.description.clone(), source_language)
}

/// 读取时类别归一：支持集合外（含 legacy）一律 `self_definition`。
pub fn normalize_category_read(raw: &str) -> String {
    let normalized = raw.trim().to_lowercase();
    if SUPPORTED_CATEGORIES.contains(&normalized.as_str()) {
        normalized
    } else {
        "self_definition".to_string()
    }
}

/// 写入/查询时类别归一：legacy 映射为 `self_definition`，未知报 400。
pub fn normalize_category_query(raw: &str) -> Result<String, ApiError> {
    let normalized = raw.trim().to_lowercase();
    if LEGACY_TO_SELF_DEFINITION.contains(&normalized.as_str()) {
        return Ok("self_definition".to_string());
    }
    if SUPPORTED_CATEGORIES.contains(&normalized.as_str()) {
        return Ok(normalized);
    }
    Err(ApiError::BadRequest(format!(
        "不支持的 category：{raw}，仅支持：{}",
        SUPPORTED_CATEGORIES.join(", ")
    )))
}

/// 按指定时区实时计算 `isActive`。
pub fn compute_is_active(
    start: Option<&str>,
    end: Option<&str>,
    stored: bool,
    zone: Tz,
    now: DateTime<Utc>,
) -> bool {
    compute_is_active_at(start, end, stored, now.with_timezone(&zone).time())
}

/// 纯函数版本：`start == end` 表示全天；跨午夜取补集；单边/非法回退数据库 `is_active`。
pub fn compute_is_active_at(
    start: Option<&str>,
    end: Option<&str>,
    stored: bool,
    now: NaiveTime,
) -> bool {
    let (Some(start), Some(end)) = (start, end) else {
        return stored;
    };
    if start.trim().is_empty() || end.trim().is_empty() {
        return stored;
    }
    let (Ok(start), Ok(end)) = (
        NaiveTime::parse_from_str(start, "%H:%M"),
        NaiveTime::parse_from_str(end, "%H:%M"),
    ) else {
        return stored;
    };
    if start == end {
        true
    } else if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compute_is_active_at, hash_parts, normalize, normalize_category_query,
        normalize_category_read, parse_accept_language, source_hash, translation_is_current,
    };
    use crate::modules::markers::model::{MarkerRow, TranslationRow};
    use chrono::{NaiveTime, Utc};

    fn time(value: &str) -> NaiveTime {
        NaiveTime::parse_from_str(value, "%H:%M").unwrap()
    }

    fn marker_row(
        id: i64,
        source_language: &str,
        title: &str,
        description: Option<&str>,
    ) -> MarkerRow {
        MarkerRow {
            deactivated: false,
            id,
            version: 0,
            lat: 0.0,
            lng: 0.0,
            category: "accessible_toilet".to_string(),
            title: title.to_string(),
            description: description.map(str::to_string),
            source_language: source_language.to_string(),
            is_public: true,
            username: "tester".to_string(),
            user_public_id: None,
            client_request_id: None,
            is_active: true,
            open_time_start: None,
            open_time_end: None,
            review_status: "APPROVED".to_string(),
            last_edited_by: None,
            last_edited_by_public_id: None,
            last_edited_by_owner: true,
            mark_image: None,
            venue_type: Some("other".to_string()),
            categories: vec!["accessible_toilet".into()],
            opening_hours_note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn translation_row(
        marker_id: i64,
        language: &str,
        title: &str,
        description: Option<&str>,
        source_hash: &str,
    ) -> TranslationRow {
        TranslationRow {
            marker_id,
            language: language.to_string(),
            title: title.to_string(),
            description: description.map(str::to_string),
            source_hash: source_hash.to_string(),
        }
    }

    #[test]
    fn accept_language_matches_java_language_range_rules() {
        for (header, expected) in [
            ("en;q=0,zh;q=0.5,en;q=1", "zh"),
            ("en--x", "zh"),
            ("en;garbage", "zh"),
            ("en;q=0.5;q=0.7", "zh"),
            ("fr, zh;q=0, en-US;q=0.7", "en"),
            ("en-US,en;q=0.9", "en"),
            ("en;q=broken", "zh"),
            ("fr,zh,en", "zh"),
            ("en;q=0.5,fr;q=0.5,zh;q=0.5", "en"),
            ("en,,zh", "zh"),
            (" ,en", "zh"),
            (",", "zh"),
            ("", "zh"),
            ("en;q=1,zh;q=1,en", "en"),
            ("en;q=0.2,en;q=0.9", "en"),
            ("en;Q=0.5", "en"),
            ("en;q=", "zh"),
            ("en;q=0.5;", "zh"),
            (";en", "zh"),
            ("en;q=1.1", "zh"),
            ("en;q=-1", "zh"),
            ("en; q = 0.5", "en"),
            ("en;q=.5", "en"),
            ("en;q=1e0", "en"),
            ("en ; q=0.5", "en"),
            ("1-2", "zh"),
            ("abcdefghi", "zh"),
            ("*", "zh"),
            (",en", "zh"),
            ("en,", "en"),
            ("zh-CN", "zh"),
            ("en;q=0,zh", "zh"),
        ] {
            assert_eq!(
                parse_accept_language(header),
                expected,
                "Accept-Language 解析不一致: {header}"
            );
        }
    }

    #[test]
    fn translation_requires_matching_marker_id() {
        let row = marker_row(1, "zh", "原文", Some("描述"));
        let hash = source_hash(&row);
        assert!(translation_is_current(
            &row,
            &translation_row(1, "en", "English", Some("desc"), &hash)
        ));
        assert!(
            !translation_is_current(
                &row,
                &translation_row(2, "en", "English", Some("desc"), &hash)
            ),
            "不同点位的相同哈希译文必须被拒绝"
        );
    }

    #[test]
    fn normalize_accepts_exact_prefixes_only() {
        assert_eq!(normalize(Some("en-US")), "en");
        assert_eq!(normalize(Some("zh-Hant")), "zh");
        assert_eq!(normalize(Some("EN")), "en");
        assert_eq!(normalize(Some("en_US")), "en");
        assert_eq!(normalize(Some("fr")), "zh");
        assert_eq!(normalize(Some("system")), "zh");
        assert_eq!(normalize(Some("*")), "zh");
        assert_eq!(normalize(Some("")), "zh");
        assert_eq!(normalize(None), "zh");
    }

    #[test]
    fn canonical_hash_matches_python_including_unicode_escapes_and_null_text() {
        assert_eq!(
            hash_parts("zh", None, None),
            "3defae280890f55c022bcd5c252977969064fb6328f2927428ba4a0b02e14840"
        );
        assert_eq!(
            hash_parts("zh", Some("中\"文\\\n😀"), Some("line\t\u{1f}")),
            "69041ef21862545c32d414958ea2e68ef0287b0b67807d93ebfce3179dae8e87"
        );
        // 非 ASCII 不转义：显式 UTF-8 字节参与哈希。
        assert_ne!(
            hash_parts("zh", Some("中文"), None),
            hash_parts("zh", Some("\\u4e2d\\u6587"), None)
        );
    }

    #[test]
    fn category_normalization_matches_java() {
        assert_eq!(normalize_category_read("SAFE_PLACE"), "self_definition");
        assert_eq!(
            normalize_category_read("accessible_toilet"),
            "accessible_toilet"
        );
        assert_eq!(normalize_category_read("legacy"), "self_definition");
        assert_eq!(
            normalize_category_query("dangerous_place").unwrap(),
            "self_definition"
        );
        assert_eq!(
            normalize_category_query(" Baby_Room ").unwrap(),
            "baby_room"
        );
        assert!(normalize_category_query("unknown").is_err());
    }

    #[test]
    fn availability_handles_full_day_cross_midnight_and_single_sided_data() {
        // 单边数据回退数据库 is_active。
        assert!(!compute_is_active_at(
            Some("08:00"),
            None,
            false,
            time("12:00")
        ));
        assert!(compute_is_active_at(
            None,
            Some("08:00"),
            true,
            time("23:00")
        ));
        // 单点窗口视为全天。
        assert!(compute_is_active_at(
            Some("12:00"),
            Some("12:00"),
            false,
            time("03:00")
        ));
        // 常规窗口与跨午夜窗口。
        assert!(compute_is_active_at(
            Some("08:00"),
            Some("20:00"),
            false,
            time("08:00")
        ));
        assert!(!compute_is_active_at(
            Some("08:00"),
            Some("20:00"),
            false,
            time("20:00")
        ));
        assert!(compute_is_active_at(
            Some("22:00"),
            Some("06:00"),
            false,
            time("23:30")
        ));
        assert!(compute_is_active_at(
            Some("22:00"),
            Some("06:00"),
            false,
            time("05:59")
        ));
        assert!(!compute_is_active_at(
            Some("22:00"),
            Some("06:00"),
            false,
            time("12:00")
        ));
        // 非法时间回退数据库值。
        assert!(compute_is_active_at(
            Some("bad"),
            Some("06:00"),
            true,
            time("12:00")
        ));
    }
}
