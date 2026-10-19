SELECT p.id,
       p.marker_id,
       p.marker_title,
       p.status,
       p.review_note,
       p.created_at,
       p.reviewed_at
FROM marker_edit_proposals p
JOIN map_markers m ON m.id = p.marker_id
WHERE p.proposer_public_id = $1 AND m.deactivated = false
