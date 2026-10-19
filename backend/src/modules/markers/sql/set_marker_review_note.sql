UPDATE map_markers
SET review_note = $2
WHERE id = $1
