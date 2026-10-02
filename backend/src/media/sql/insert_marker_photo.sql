INSERT INTO marker_photos(marker_id, image_url, sort_order)
VALUES ($1, $2, $3) ON CONFLICT (marker_id, image_url) DO NOTHING
