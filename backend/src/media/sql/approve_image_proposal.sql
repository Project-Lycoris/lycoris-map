UPDATE marker_image_proposals
SET status = 'APPROVED',
    reviewed_by = $1,
    reviewed_at = now(),
    review_note = NULL
WHERE id = $2
  AND status = 'PENDING'
