SELECT
    id, version, lat, lng, category, title, description, source_language,
    is_public, username, user_public_id, client_request_id, is_active,
    open_time_start, open_time_end, review_status, last_edited_by,
    last_edited_by_public_id, last_edited_by_owner, mark_image, venue_type, categories, opening_hours_note, deactivated, created_at, updated_at
FROM map_markers m
WHERE m.is_public = true
  AND m.review_status = 'APPROVED'
  AND m.deactivated = false
  AND (
    lower(m.title) LIKE lower('%' || $1 || '%')
    OR lower(m.description) LIKE lower('%' || $1 || '%')
    OR lower(m.category) LIKE lower('%' || $1 || '%')
    OR m.lat::text LIKE '%' || $1 || '%'
    OR m.lng::text LIKE '%' || $1 || '%'
  )
ORDER BY m.id ASC
