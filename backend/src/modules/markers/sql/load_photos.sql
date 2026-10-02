SELECT id, marker_id, image_url AS url, sort_order FROM marker_photos WHERE marker_id = ANY($1::bigint[]) ORDER BY marker_id, sort_order, id
