UPDATE map_markers
SET last_deactivated_version = CASE WHEN $2 THEN version + 1 ELSE last_deactivated_version END,
    deactivated = $2, version = version + 1, updated_at = now()
WHERE id = $1 AND deactivated <> $2
