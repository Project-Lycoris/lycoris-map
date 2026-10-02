UPDATE map_markers SET
    category = $2,
    title = $3,
    description = $4,
    source_language = $5,
    is_public = $6,
    is_active = $7,
    open_time_start = $8,
    open_time_end = $9,
    review_status = $10,
    last_edited_by = $11,
    last_edited_by_public_id = $12,
    last_edited_by_owner = $13,
    venue_type = $14,
    categories = $15,
    opening_hours_note = $16,
    updated_at = now(),
    version = version + 1
WHERE id = $1
RETURNING
    id, version, lat, lng, category, title, description, source_language,
    is_public, username, user_public_id, client_request_id, is_active,
    open_time_start, open_time_end, review_status, last_edited_by,
    last_edited_by_public_id, last_edited_by_owner, mark_image, venue_type, categories, opening_hours_note, deactivated, created_at, updated_at
