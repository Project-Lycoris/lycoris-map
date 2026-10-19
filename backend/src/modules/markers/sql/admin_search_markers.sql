SELECT
    id, version, lat, lng, category, title, description, source_language,
    is_public, username, user_public_id, client_request_id, is_active,
    open_time_start, open_time_end, review_status, last_edited_by,
    last_edited_by_public_id, last_edited_by_owner, mark_image, venue_type,
    categories, opening_hours_note, deactivated, created_at, updated_at
FROM map_markers
WHERE ($1::text IS NULL OR (
        title ILIKE $1 ESCAPE '\'
        OR coalesce(description, '') ILIKE $1 ESCAPE '\'
        OR username ILIKE $1 ESCAPE '\'
        OR ($2::bigint IS NOT NULL AND id = $2)
    ))
  AND ($3::text IS NULL OR review_status = $3)
  AND ($4::text IS NULL OR $4 = ANY(categories))
  AND ($5::boolean IS NULL OR deactivated = $5)
ORDER BY
    CASE WHEN $6 = 'updated_desc' THEN updated_at END DESC NULLS LAST,
    CASE WHEN $6 = 'created_desc' THEN created_at END DESC NULLS LAST,
    CASE WHEN $6 = 'created_asc' THEN created_at END ASC NULLS LAST,
    id DESC
LIMIT $7 OFFSET $8
