UPDATE map_markers
SET mark_image = COALESCE((SELECT image_url FROM marker_photos WHERE marker_id = $2 ORDER BY sort_order, id LIMIT 1), $1),
    version = version + 1,
    updated_at = now()
WHERE id = $2 AND deactivated = false
RETURNING
    id, version, lat, lng, category, title, description, source_language,
    is_public, username, user_public_id, client_request_id, is_active,
    open_time_start, open_time_end, review_status, last_edited_by,
    last_edited_by_public_id, last_edited_by_owner, mark_image, venue_type, categories, opening_hours_note, deactivated, created_at, updated_at
