SELECT id,
       id AS marker_id,
       title AS marker_title,
       review_status AS status,
       review_note,
       created_at,
       CASE WHEN review_status = 'PENDING' THEN NULL ELSE updated_at END AS reviewed_at
FROM map_markers
WHERE user_public_id = $1 AND deactivated = false
