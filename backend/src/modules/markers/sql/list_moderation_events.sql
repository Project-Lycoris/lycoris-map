SELECT id, created_at, actor_public_id, actor_username, action,
       target_kind, target_id, marker_id, note
FROM moderation_events
WHERE ($1::bigint IS NULL OR marker_id = $1)
ORDER BY created_at DESC, id DESC
LIMIT $2 OFFSET $3
