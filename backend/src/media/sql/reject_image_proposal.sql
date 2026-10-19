UPDATE marker_image_proposals
SET status = 'REJECTED',
    reviewed_by = $1,
    reviewed_at = now(),
    review_note = $3
WHERE id = $2
  AND status = 'PENDING'
