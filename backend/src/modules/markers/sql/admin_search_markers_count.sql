SELECT count(*)
FROM map_markers
WHERE ($1::text IS NULL OR (
        title ILIKE $1 ESCAPE '\'
        OR coalesce(description, '') ILIKE $1 ESCAPE '\'
        OR username ILIKE $1 ESCAPE '\'
        OR ($2::bigint IS NOT NULL AND id = $2)
    ))
  AND ($3::text IS NULL OR review_status = $3)
  AND ($4::text IS NULL OR $4 = ANY(categories))
  AND ($5::boolean IS NULL OR deactivated = $5)
