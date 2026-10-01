-- Current contributions are Chinese regardless of the submitting client's UI.
-- Published content and manual translations are kept intact.
UPDATE map_markers SET source_language = 'zh' WHERE review_status = 'PENDING';
UPDATE marker_edit_proposals SET language = 'zh' WHERE status = 'PENDING';

-- Keep old proposals stale across disable/restore, even if text is unchanged.
ALTER TABLE map_markers ADD COLUMN last_deactivated_version bigint NOT NULL DEFAULT 0;
ALTER TABLE marker_edit_proposals ADD COLUMN base_content jsonb;
-- Only backfill a provably current baseline. Never bless an already stale proposal.
UPDATE marker_edit_proposals p SET base_content = jsonb_build_object(
    'category', m.category, 'title', m.title, 'description', m.description,
    'source_language', m.source_language, 'lat', encode(float8send(m.lat), 'hex'), 'lng', encode(float8send(m.lng), 'hex'),
    'is_public', m.is_public, 'is_active', m.is_active,
    'open_time_start', m.open_time_start, 'open_time_end', m.open_time_end,
    'venue_type', m.venue_type, 'deactivated', m.deactivated,
    'last_deactivated_version', m.last_deactivated_version
)
FROM map_markers m
WHERE p.marker_id = m.id AND p.status = 'PENDING' AND p.base_marker_version = m.version;
