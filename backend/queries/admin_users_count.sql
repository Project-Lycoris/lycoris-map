SELECT count(*)
FROM users
WHERE ($1::text IS NULL OR username ILIKE $1 ESCAPE '\'
       OR nickname ILIKE $1 ESCAPE '\' OR email ILIKE $1 ESCAPE '\'
       OR ($2::bigint IS NOT NULL AND id = $2))
