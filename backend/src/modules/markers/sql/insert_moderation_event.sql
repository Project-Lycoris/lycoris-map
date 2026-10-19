INSERT INTO moderation_events
    (actor_public_id, actor_username, action, target_kind, target_id, marker_id, note)
VALUES ($1, $2, $3, $4, $5, $6, $7)
