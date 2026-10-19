SELECT count(*)
FROM moderation_events
WHERE ($1::bigint IS NULL OR marker_id = $1)
