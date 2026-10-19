SELECT id, public_id, username, nickname, email, password, avatar_url, pronouns, signature,
       role, deleted, deleted_at, session_version, row_version
FROM users
WHERE ($1::text IS NULL OR username ILIKE $1 ESCAPE '\'
       OR nickname ILIKE $1 ESCAPE '\' OR email ILIKE $1 ESCAPE '\'
       OR ($2::bigint IS NOT NULL AND id = $2))
ORDER BY id DESC
LIMIT $3 OFFSET $4
