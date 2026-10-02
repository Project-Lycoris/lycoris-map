-- Reviewed against existing Chinese source text on 2026-10-02. Station names
-- (Tianhe/Changhong/Olympic Park), POPARK mall and a deactivated duplicate are
-- deliberately excluded. Exact title/version guards leave subsequently edited
-- records and unrelated databases unchanged.
UPDATE map_markers AS m
SET venue_type = 'park', version = m.version + 1, updated_at = now()
FROM (VALUES
  (37::bigint, 2::bigint, '甘井子区-三道沟东联路桥-公共卫生间(毋分性别设计)'),
  (220, 1, '海淀区-马甸公园(北)-无障碍卫生间'),
  (332, 1, '和平区-复兴公园公厕(HP1-WDD19)-无障碍卫生间'),
  (371, 2, '翔安区-洪前东山里90-2-公共卫生间(毋分性别设计)'),
  (409, 2, '浦东新区-汤巷公园(西)-无障碍卫生间'),
  (413, 7, '滨海新区(塘沽)-苏州里公园-公厕特需室'),
  (416, 4, '滨海新区(塘沽)-文明里(滨海民生市场)-公厕特需室'),
  (437, 2, '南开区-凤湖公园公厕(NK2-XF03)-无障碍卫生间')
) AS reviewed(id, version, title)
WHERE m.id = reviewed.id AND m.version = reviewed.version AND m.title = reviewed.title
  AND m.source_language = 'zh' AND NOT m.deactivated
  AND 'accessible_toilet' = ANY(m.categories)
  AND m.venue_type IN ('public_toilet', 'other');
