INSERT INTO marker_edit_proposals (
    marker_id, marker_title, marker_lat, marker_lng, proposer_username, proposer_public_id,
    proposer_is_owner, category, title, description, language, is_public, is_active,
    open_time_start, open_time_end, venue_type, base_marker_version, base_content, status, created_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, 'PENDING', now())
