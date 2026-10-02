SELECT
    id, marker_id, marker_title, marker_lat, marker_lng, category, title, description,
    language, is_public, is_active, open_time_start, open_time_end, venue_type, categories, opening_hours_note,
    proposer_username, proposer_public_id, proposer_is_owner, status,
    base_marker_version, base_content, created_at
FROM marker_edit_proposals
WHERE id = $1
FOR UPDATE
