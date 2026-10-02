//! 固定 SQL 仓储层。
//!
//! 所有查询都来自 `sql/` 下的 `.sql` 文件并使用 `query_file_as!` 编译期校验，
//! 便于生成 `.sqlx` 离线元数据并在无数据库环境构建。查询参数全部绑定，不拼接用户值。

use sqlx::PgPool;

use crate::modules::markers::model::{MarkerPhoto, MarkerRow, TranslationRow};

/// `map_markers` 读取仓储。
#[derive(Debug, Clone)]
pub struct MarkerRepository {
    pool: PgPool,
}

impl MarkerRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn load_photos(&self, ids: &[i64]) -> Result<Vec<MarkerPhoto>, sqlx::Error> {
        sqlx::query_file_as!(MarkerPhoto, "src/modules/markers/sql/load_photos.sql", ids)
            .fetch_all(&self.pool)
            .await
    }

    /// 公开且已审核的全部点位。
    pub async fn find_public(&self) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(MarkerRow, "src/modules/markers/sql/find_public.sql")
            .fetch_all(&self.pool)
            .await
    }

    /// 按主键读取单行（不做可见性过滤，由调用方判定）。
    pub async fn find_by_id(&self, id: i64) -> Result<Option<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(MarkerRow, "src/modules/markers/sql/find_by_id.sql", id)
            .fetch_optional(&self.pool)
            .await
    }

    /// 按主键批量读取当前公开且已审核的点位。
    pub async fn find_public_by_ids(&self, ids: &[i64]) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_public_by_ids.sql",
            ids
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 原文/类别/坐标文本模糊匹配。
    pub async fn search_source(&self, query: &str) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/search_source.sql",
            query
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 译文模糊匹配（哈希有效性由服务层校验）。
    pub async fn search_translations(
        &self,
        query: &str,
    ) -> Result<Vec<TranslationRow>, sqlx::Error> {
        sqlx::query_file_as!(
            TranslationRow,
            "src/modules/markers/sql/search_translations.sql",
            query
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 可解析坐标的容差匹配。
    pub async fn search_coordinates(
        &self,
        lat: f64,
        lng: f64,
        eps: f64,
    ) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/search_coordinates.sql",
            lat,
            lng,
            eps
        )
        .fetch_all(&self.pool)
        .await
    }

    /// PostGIS 候选 + legacy 有限异常坐标的 Haversine 邻近查询；参数语义与 Java 仓储一致。
    pub async fn find_nearby(
        &self,
        lat: f64,
        lng: f64,
        radius_meters: f64,
        category: &str,
    ) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_nearby.sql",
            lat,
            lng,
            radius_meters,
            category
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 视口矩形内公开点位。
    pub async fn find_viewport(
        &self,
        min_lat: f64,
        max_lat: f64,
        min_lng: f64,
        max_lng: f64,
    ) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_viewport.sql",
            min_lat,
            max_lat,
            min_lng,
            max_lng
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 视口矩形内、指定类别集合的公开点位。
    pub async fn find_viewport_categories(
        &self,
        min_lat: f64,
        max_lat: f64,
        min_lng: f64,
        max_lng: f64,
        categories: &[String],
    ) -> Result<Vec<MarkerRow>, sqlx::Error> {
        sqlx::query_file_as!(
            MarkerRow,
            "src/modules/markers/sql/find_viewport_categories.sql",
            min_lat,
            max_lat,
            min_lng,
            max_lng,
            categories
        )
        .fetch_all(&self.pool)
        .await
    }

    /// 批量加载指定语言的译文，避免 N+1。
    pub async fn load_translations(
        &self,
        ids: &[i64],
        language: &str,
    ) -> Result<Vec<TranslationRow>, sqlx::Error> {
        sqlx::query_file_as!(
            TranslationRow,
            "src/modules/markers/sql/load_translations.sql",
            ids,
            language
        )
        .fetch_all(&self.pool)
        .await
    }
}
