-- 邻近查询：PostGIS 空间候选 + legacy 有限异常坐标并集，最终按原 Haversine 精确筛选。
--
-- 参数：$1=lat, $2=lng, $3=radius_meters, $4=category（与 Java 仓储语义一致，半径由服务层夹取）。
--
-- 设计：
--   * 合法行（location 非空）用 GiST 部分索引做保守候选：候选半径 radius*1.002+0.01 m，
--     覆盖 WGS84 最大半轴与产品球半径 6371000 m 的比值，再按原半径精确筛选，不扩大结果；
--   * location 为 NULL 的有限历史越界坐标走 legacy 分支，按原 Haversine 公式（含周期折回）
--     在最终筛选中处理；两条分支用 UNION ALL，避免跨分支 OR 使空间索引失效；
--   * 距离公式在三角函数前显式排除 NaN/Infinity，且把 asin 的入参 clamp 到 [0,1]，
--     防止异常行或浮点极值造成 asin 域错误；不增加 LIMIT/任意截断，保持 distance,id 稳定排序；
--   * 最终列仍从 map_markers 直接选取（JOIN 候选 ID），避免子查询丢失 NOT NULL 元数据，
--     也不把 SQLx 不支持的 geography 生成列带入结果。
WITH candidates AS (
    SELECT
        m.id,
        CASE
            WHEN m.lat > '-Infinity'::double precision
                AND m.lat < 'Infinity'::double precision
                AND m.lng > '-Infinity'::double precision
                AND m.lng < 'Infinity'::double precision
            THEN 6371000 * 2 * asin(sqrt(
                    LEAST(1.0, GREATEST(0.0,
                        power(sin(radians((m.lat - $1::double precision) / 2)), 2)
                        + cos(radians($1::double precision)) * cos(radians(m.lat))
                        * power(sin(radians((m.lng - $2::double precision) / 2)), 2)
                    ))
                 ))
            ELSE NULL
        END AS distance
    FROM public.map_markers m
    WHERE m.is_public = true
      AND m.review_status = 'APPROVED'
      AND m.deactivated = false
      AND $4::text = ANY(m.categories)
      AND m.location IS NOT NULL
      AND ST_DWithin(
            m.location,
            ST_SetSRID(ST_MakePoint($2::double precision, $1::double precision), 4326)::geography,
            $3::double precision * 1.002 + 0.01,
            false
          )
    UNION ALL
    SELECT
        m.id,
        CASE
            WHEN m.lat > '-Infinity'::double precision
                AND m.lat < 'Infinity'::double precision
                AND m.lng > '-Infinity'::double precision
                AND m.lng < 'Infinity'::double precision
            THEN 6371000 * 2 * asin(sqrt(
                    LEAST(1.0, GREATEST(0.0,
                        power(sin(radians((m.lat - $1::double precision) / 2)), 2)
                        + cos(radians($1::double precision)) * cos(radians(m.lat))
                        * power(sin(radians((m.lng - $2::double precision) / 2)), 2)
                    ))
                 ))
            ELSE NULL
        END AS distance
    FROM public.map_markers m
    WHERE m.is_public = true
      AND m.review_status = 'APPROVED'
      AND m.deactivated = false
      AND $4::text = ANY(m.categories)
      AND m.location IS NULL
)
SELECT
    m.id, m.version, m.lat, m.lng, m.category, m.title, m.description,
    m.source_language, m.is_public, m.username, m.user_public_id,
    m.client_request_id, m.is_active, m.open_time_start, m.open_time_end,
    m.review_status, m.last_edited_by, m.last_edited_by_public_id,
    m.last_edited_by_owner, m.mark_image, m.venue_type, m.categories, m.opening_hours_note, m.deactivated, m.created_at, m.updated_at
FROM public.map_markers m
JOIN candidates c ON c.id = m.id
WHERE c.distance <= $3::double precision
ORDER BY c.distance ASC, m.id ASC;
