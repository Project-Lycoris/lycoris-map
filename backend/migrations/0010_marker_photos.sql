-- Published albums are separate from the moderation history.
CREATE TABLE marker_photos (
    id bigserial PRIMARY KEY,
    marker_id bigint NOT NULL REFERENCES map_markers(id) ON DELETE CASCADE,
    image_url text NOT NULL,
    sort_order bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(marker_id, image_url)
);
CREATE INDEX ix_marker_photos_order ON marker_photos(marker_id, sort_order, id);
CREATE INDEX ix_marker_photos_url ON marker_photos(image_url);
INSERT INTO marker_photos(marker_id, image_url, sort_order)
SELECT id, mark_image, 0 FROM map_markers WHERE mark_image IS NOT NULL AND btrim(mark_image) <> '';
