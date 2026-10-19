UPDATE marker_edit_proposals
SET status = $2,
    reviewed_by = $3,
    reviewed_at = $4,
    review_note = $5
WHERE id = $1
