SELECT e.id, e.created_at, e.actor_public_id, e.actor_username, e.action,
       e.target_kind, e.target_id, e.marker_id,
       CASE WHEN m.id IS NOT NULL THEN m.title END AS marker_title,
       CASE WHEN e.target_kind = 'user' THEN u.username END AS target_label,
       e.note
FROM moderation_events e
LEFT JOIN map_markers m ON m.id = e.marker_id
LEFT JOIN users u ON e.target_kind = 'user' AND u.id = e.target_id
WHERE ($1::bigint IS NULL OR e.marker_id = $1)
ORDER BY e.created_at DESC, e.id DESC
LIMIT $2 OFFSET $3
