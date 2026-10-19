SELECT coalesce(public_id, '')::text AS public_id,
       coalesce(count(*) FILTER (WHERE status = 'APPROVED'), 0)::bigint AS approved,
       coalesce(count(*) FILTER (WHERE status = 'REJECTED'), 0)::bigint AS rejected,
       coalesce(count(*) FILTER (WHERE status = 'PENDING'), 0)::bigint AS pending
FROM (
    SELECT user_public_id AS public_id, review_status AS status
    FROM map_markers
    WHERE user_public_id = ANY($1)
    UNION ALL
    SELECT proposer_public_id, status
    FROM marker_edit_proposals
    WHERE proposer_public_id = ANY($1)
    UNION ALL
    SELECT proposer_public_id, status
    FROM marker_image_proposals
    WHERE proposer_public_id = ANY($1)
) submissions
WHERE public_id IS NOT NULL
GROUP BY public_id
