SELECT
    p.proposer_public_id,
    m.id, m.version, m.lat, m.lng, m.category, m.title, m.description, m.source_language,
    m.is_public, m.username, m.user_public_id, m.client_request_id, m.is_active,
    m.open_time_start, m.open_time_end, m.review_status, m.last_edited_by,
    m.last_edited_by_public_id, m.last_edited_by_owner, m.mark_image, m.venue_type, m.categories, m.opening_hours_note, m.deactivated, m.created_at, m.updated_at
FROM marker_image_proposals p
INNER JOIN map_markers m ON m.id = p.marker_id
WHERE p.image_url = $1
ORDER BY p.id ASC
