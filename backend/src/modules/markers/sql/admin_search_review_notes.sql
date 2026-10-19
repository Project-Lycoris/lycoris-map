SELECT id, review_note
FROM map_markers
WHERE id = ANY($1)
