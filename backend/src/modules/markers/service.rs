//! 公开点位读取业务编排。
//!
//! 职责：可见性判定、类别归一、搜索合并去重、批量本地化、Redis ID 缓存回源。
//! 读取只对副本计算，不回写数据库、不推进 `version`。

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use chrono_tz::Tz;

use crate::error::ApiError;
use crate::modules::markers::cache::{MarkerCache, NEARBY_TTL_SECONDS, VIEWPORT_TTL_SECONDS};
use crate::modules::markers::localization::{
    compute_is_active, localize_text, normalize_category_query, normalize_category_read,
    translation_is_current,
};
use crate::modules::markers::model::{MarkerDto, MarkerRow, TranslationRow, Viewer, can_view};
use crate::modules::markers::repository::MarkerRepository;

/// 坐标容差（度）。
const COORDINATE_EPSILON: f64 = 0.00015;

/// 点位读取服务。
#[derive(Clone)]
pub struct MarkerService {
    repo: MarkerRepository,
    cache: MarkerCache,
    zone: Tz,
}

impl MarkerService {
    pub fn new(repo: MarkerRepository, cache: MarkerCache, zone: Tz) -> Self {
        Self { repo, cache, zone }
    }

    /// `GET /api/markers/public`。
    pub async fn list_public(&self, lang: &'static str) -> Result<Vec<MarkerDto>, ApiError> {
        let rows = self.repo.find_public().await.map_err(db_error)?;
        self.localize_rows(rows, lang).await
    }

    /// `GET /api/markers/search`。空白 `q` 返回空数组。
    pub async fn search(
        &self,
        q: Option<&str>,
        lang: &'static str,
    ) -> Result<Vec<MarkerDto>, ApiError> {
        let query = q.unwrap_or("").trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut ordered: Vec<MarkerRow> = Vec::new();
        let mut seen: HashSet<i64> = HashSet::new();

        for row in self.repo.search_source(query).await.map_err(db_error)? {
            insert_unique(&mut ordered, &mut seen, row);
        }

        let translations = self
            .repo
            .search_translations(query)
            .await
            .map_err(db_error)?;
        if !translations.is_empty() {
            let mut ids: Vec<i64> = Vec::new();
            for translation in &translations {
                if !ids.contains(&translation.marker_id) {
                    ids.push(translation.marker_id);
                }
            }
            let markers = self.repo.find_public_by_ids(&ids).await.map_err(db_error)?;
            let by_id: HashMap<i64, MarkerRow> =
                markers.into_iter().map(|row| (row.id, row)).collect();
            for translation in &translations {
                if let Some(row) = by_id.get(&translation.marker_id)
                    && translation_is_current(row, translation)
                {
                    insert_unique(&mut ordered, &mut seen, row.clone());
                }
            }
        }

        if let Some((lat, lng)) = parse_lat_lng(query) {
            for row in self
                .repo
                .search_coordinates(lat, lng, COORDINATE_EPSILON)
                .await
                .map_err(db_error)?
            {
                insert_unique(&mut ordered, &mut seen, row);
            }
        }

        self.localize_rows(ordered, lang).await
    }

    /// `GET /api/markers/nearby`。
    pub async fn nearby(
        &self,
        lat: f64,
        lng: f64,
        radius: i32,
        category: &str,
        lang: &'static str,
    ) -> Result<Vec<MarkerDto>, ApiError> {
        if !lat.is_finite()
            || !lng.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lng)
        {
            return Err(ApiError::BadRequest("lat/lng 不合法".to_string()));
        }
        let category = normalize_category_query(category)?;
        let safe_radius = radius.clamp(1, 50_000);

        let cache_key = self.cache.current_generation().await.map(|generation| {
            self.cache
                .nearby_key(generation, lat, lng, safe_radius, &category)
        });

        if let Some(key) = &cache_key
            && let Some(ids) = self.cache.read_ids(key).await
        {
            let rows = self.repo.find_public_by_ids(&ids).await.map_err(db_error)?;
            let ordered = order_by_ids(&ids, rows);
            return self.localize_rows(ordered, lang).await;
        }

        let rows = self
            .repo
            .find_nearby(lat, lng, safe_radius as f64, &category)
            .await
            .map_err(db_error)?;

        if let Some(key) = &cache_key {
            let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
            self.cache.write_ids(key, &ids, NEARBY_TTL_SECONDS).await;
        }
        self.localize_rows(rows, lang).await
    }

    /// `GET /api/markers/viewport`。
    pub async fn viewport(
        &self,
        min_lat: f64,
        max_lat: f64,
        min_lng: f64,
        max_lng: f64,
        categories_raw: Option<&str>,
        lang: &'static str,
    ) -> Result<Vec<MarkerDto>, ApiError> {
        if ![min_lat, max_lat, min_lng, max_lng]
            .iter()
            .all(|value| value.is_finite())
        {
            return Err(ApiError::BadRequest("边界超出合法经纬度范围".to_string()));
        }
        if min_lat > max_lat || min_lng > max_lng {
            return Err(ApiError::BadRequest("边界参数不合法".to_string()));
        }
        if min_lat < -90.0 || max_lat > 90.0 || min_lng < -180.0 || max_lng > 180.0 {
            return Err(ApiError::BadRequest("边界超出合法经纬度范围".to_string()));
        }

        let categories = match categories_raw {
            Some(raw) if !raw.trim().is_empty() => {
                let mut normalized = Vec::new();
                for part in raw.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    normalized.push(normalize_category_query(part)?);
                }
                if normalized.is_empty() {
                    None
                } else {
                    Some(normalized)
                }
            }
            _ => None,
        };
        let category_part = category_cache_part(categories.as_deref());

        let cache_key = self.cache.current_generation().await.map(|generation| {
            self.cache.viewport_key(
                generation,
                min_lat,
                max_lat,
                min_lng,
                max_lng,
                &category_part,
            )
        });

        if let Some(key) = &cache_key
            && let Some(ids) = self.cache.read_ids(key).await
        {
            let rows = self.repo.find_public_by_ids(&ids).await.map_err(db_error)?;
            let ordered = order_by_ids(&ids, rows);
            return self.localize_rows(ordered, lang).await;
        }

        let rows = match categories.as_deref() {
            Some(categories) => self
                .repo
                .find_viewport_categories(min_lat, max_lat, min_lng, max_lng, categories)
                .await
                .map_err(db_error)?,
            None => self
                .repo
                .find_viewport(min_lat, max_lat, min_lng, max_lng)
                .await
                .map_err(db_error)?,
        };

        if let Some(key) = &cache_key {
            let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
            self.cache.write_ids(key, &ids, VIEWPORT_TTL_SECONDS).await;
        }
        self.localize_rows(rows, lang).await
    }

    /// `GET /api/markers/{id}`。不可见返回 `None`（对外 404）。
    pub async fn detail(
        &self,
        id: i64,
        viewer: Option<&Viewer<'_>>,
        lang: &'static str,
    ) -> Result<Option<MarkerDto>, ApiError> {
        let Some(row) = self.repo.find_by_id(id).await.map_err(db_error)? else {
            return Ok(None);
        };
        if !can_view(&row, viewer) {
            return Ok(None);
        }
        Ok(self
            .localize_rows(vec![row], lang)
            .await?
            .into_iter()
            .next())
    }

    /// 供写入核心复用：把最新 `MarkerRow` 批量本地化为响应 DTO（含读取期类别归一与 `isActive`）。
    ///
    /// 写接口（创建/审核/管理员编辑）应返回数据库最新行后调用此方法，不得在此回写数据库或
    /// 把按时间计算的 `isActive` 持久化。
    pub async fn localize(
        &self,
        rows: Vec<MarkerRow>,
        lang: &'static str,
    ) -> Result<Vec<MarkerDto>, ApiError> {
        self.localize_rows(rows, lang).await
    }

    /// 本地化单个数据库行，供图片上传/审批返回本地化 `MarkerDto`。
    ///
    /// 复用与公开读取相同的译文加载与回退规则；行必须来自可信查询（如
    /// [`MediaService::submit_marker_image`](crate::media::MediaService::submit_marker_image)
    /// 返回的原点位或审批后的点位），不在此另做可见性判断。
    pub async fn localize_row(
        &self,
        row: MarkerRow,
        lang: &'static str,
    ) -> Result<MarkerDto, ApiError> {
        let mut rows = self.localize_rows(vec![row], lang).await?;
        rows.pop().ok_or(ApiError::Internal)
    }

    /// A committed mutation cannot become a failed write because an optional
    /// translation read failed. Return the saved source as the bounded fallback.
    pub async fn committed_row(&self, row: MarkerRow, lang: &'static str) -> MarkerDto {
        {
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                self.localize_row(row.clone(), lang),
            )
            .await
            {
                Ok(Ok(dto)) => return dto,
                _ => tracing::warn!("Saved marker response fell back to source text"),
            }
        }
        let language = super::localization::normalize(Some(&row.source_language));
        let category = normalize_category_read(&row.category);
        let is_active = compute_is_active(
            row.open_time_start.as_deref(),
            row.open_time_end.as_deref(),
            row.is_active,
            self.zone,
            Utc::now(),
        );
        let title = row.title.clone();
        let description = row.description.clone();
        MarkerDto::from_row(
            row,
            category,
            title,
            description,
            language,
            is_active,
            self.zone.name(),
        )
    }

    /// 批量本地化：一次加载所有译文，避免 N+1。
    async fn localize_rows(
        &self,
        rows: Vec<MarkerRow>,
        lang: &'static str,
    ) -> Result<Vec<MarkerDto>, ApiError> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i64> = rows
            .iter()
            .filter(|row| super::localization::normalize(Some(&row.source_language)) != lang)
            .map(|row| row.id)
            .collect();
        let translations = if ids.is_empty() {
            Vec::new()
        } else {
            self.repo
                .load_translations(&ids, lang)
                .await
                .map_err(db_error)?
        };
        let mut by_marker: HashMap<i64, TranslationRow> =
            HashMap::with_capacity(translations.len());
        for translation in translations {
            by_marker.insert(translation.marker_id, translation);
        }

        let all_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let mut photos: HashMap<i64, Vec<super::model::MarkerPhoto>> = HashMap::new();
        for photo in self.repo.load_photos(&all_ids).await.map_err(db_error)? {
            photos.entry(photo.marker_id).or_default().push(photo);
        }
        let now = Utc::now();
        Ok(rows
            .into_iter()
            .map(|row| {
                let translation = by_marker.get(&row.id);
                let (title, description, content_language) = localize_text(&row, translation, lang);
                let category = normalize_category_read(&row.category);
                let is_active = compute_is_active(
                    row.open_time_start.as_deref(),
                    row.open_time_end.as_deref(),
                    row.is_active,
                    self.zone,
                    now,
                );
                let album = photos.remove(&row.id).unwrap_or_default();
                let mut dto = MarkerDto::from_row(
                    row,
                    category,
                    title,
                    description,
                    content_language,
                    is_active,
                    self.zone.name(),
                );
                dto.photos = album;
                dto
            })
            .collect())
    }
}

/// 解析 `q` 为 `lat,lng`；仅接受两个数、范围合法且有限。
fn parse_lat_lng(query: &str) -> Option<(f64, f64)> {
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut index = 0;
    for character in query.char_indices() {
        if character.1 == ',' || character.1.is_whitespace() {
            if start < index {
                parts.push(&query[start..index]);
            }
            start = index + character.1.len_utf8();
        }
        index += character.1.len_utf8();
    }
    if start < query.len() {
        parts.push(&query[start..]);
    }
    if parts.len() != 2 {
        return None;
    }
    let lat: f64 = parts[0].parse().ok()?;
    let lng: f64 = parts[1].parse().ok()?;
    if !lat.is_finite() || !lng.is_finite() {
        return None;
    }
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lng) {
        return None;
    }
    Some((lat, lng))
}

fn category_cache_part(categories: Option<&[String]>) -> String {
    match categories {
        None => "all".to_string(),
        Some(categories) => {
            let mut unique = categories.to_vec();
            unique.sort();
            unique.dedup();
            unique.join(",")
        }
    }
}

fn insert_unique(ordered: &mut Vec<MarkerRow>, seen: &mut HashSet<i64>, row: MarkerRow) {
    if seen.insert(row.id) {
        ordered.push(row);
    }
}

/// 按缓存中的 ID 顺序重排数据库行（邻近按距离顺序）。
fn order_by_ids(ids: &[i64], rows: Vec<MarkerRow>) -> Vec<MarkerRow> {
    let mut by_id: HashMap<i64, MarkerRow> = rows.into_iter().map(|row| (row.id, row)).collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

fn db_error(error: sqlx::Error) -> ApiError {
    // 只记录受控 SQLSTATE，不输出底层驱动 detail、SQL 或参数。
    let code = error
        .as_database_error()
        .and_then(|database| database.code().map(|value| value.into_owned()));
    tracing::error!(
        target: "lycoris_backend::markers",
        db_code = code.as_deref().unwrap_or("none"),
        "点位数据库查询失败"
    );
    if crate::db::is_timeout_sqlstate(&error) {
        ApiError::Unavailable
    } else {
        ApiError::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::{category_cache_part, parse_lat_lng};

    #[test]
    fn coordinate_query_parses_two_finite_in_range_numbers() {
        assert_eq!(parse_lat_lng("31.2304,121.4737"), Some((31.2304, 121.4737)));
        assert_eq!(parse_lat_lng("31.2304 121.4737"), Some((31.2304, 121.4737)));
        assert_eq!(parse_lat_lng(" -33.8 , 151.2 "), Some((-33.8, 151.2)));
        assert_eq!(parse_lat_lng("90,180"), Some((90.0, 180.0)));
        assert_eq!(parse_lat_lng("91,0"), None);
        assert_eq!(parse_lat_lng("0,181"), None);
        assert_eq!(parse_lat_lng("1 2 3"), None);
        assert_eq!(parse_lat_lng("NaN,0"), None);
        assert_eq!(parse_lat_lng("abc"), None);
    }

    #[test]
    fn viewport_cache_part_sorts_and_dedups() {
        let categories = vec![
            "self_definition".to_string(),
            "baby_room".to_string(),
            "baby_room".to_string(),
        ];
        assert_eq!(
            category_cache_part(Some(&categories)),
            "baby_room,self_definition"
        );
        assert_eq!(category_cache_part(None), "all");
    }
}
