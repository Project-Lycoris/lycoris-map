-- Ordered facility types retain category as the primary, legacy-compatible value.
ALTER TABLE map_markers ADD COLUMN categories text[] NOT NULL DEFAULT '{}';
ALTER TABLE marker_edit_proposals ADD COLUMN categories text[] NOT NULL DEFAULT '{}';
ALTER TABLE map_markers ADD COLUMN opening_hours_note text CHECK (char_length(opening_hours_note) <= 1000);
ALTER TABLE marker_edit_proposals ADD COLUMN opening_hours_note text CHECK (char_length(opening_hours_note) <= 1000);
UPDATE map_markers SET categories = ARRAY[category::text];
UPDATE marker_edit_proposals SET categories = ARRAY[category::text];

-- Old clients only send category. Keep supplementary types when they edit it.
CREATE FUNCTION sync_primary_category() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF cardinality(NEW.categories) = 0 THEN
        NEW.categories := ARRAY[NEW.category::text];
    ELSIF TG_OP = 'UPDATE' AND NEW.categories = OLD.categories AND NEW.category <> OLD.category THEN
        NEW.categories := ARRAY[NEW.category::text] || ARRAY(
            SELECT value FROM unnest(OLD.categories[2:]) value WHERE value <> NEW.category::text
        );
    ELSE
        NEW.category := NEW.categories[1];
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER marker_primary_category BEFORE INSERT OR UPDATE ON map_markers
    FOR EACH ROW EXECUTE FUNCTION sync_primary_category();
CREATE TRIGGER proposal_primary_category BEFORE INSERT OR UPDATE ON marker_edit_proposals
    FOR EACH ROW EXECUTE FUNCTION sync_primary_category();
CREATE INDEX ix_map_markers_categories ON map_markers USING gin(categories)
    WHERE is_public AND review_status = 'APPROVED' AND NOT deactivated;
ALTER TABLE map_markers DROP CONSTRAINT ck_map_markers_venue_type;
ALTER TABLE marker_edit_proposals DROP CONSTRAINT ck_marker_edit_proposals_venue_type;
ALTER TABLE map_markers ADD CONSTRAINT ck_map_markers_venue_type CHECK (
    venue_type IS NULL OR (venue_type IN ('metro','hospital','mall','railway_station','school','public_toilet','airport','park','other')
        AND 'accessible_toilet' = ANY(categories)));
ALTER TABLE marker_edit_proposals ADD CONSTRAINT ck_marker_edit_proposals_venue_type CHECK (
    venue_type IS NULL OR (venue_type IN ('metro','hospital','mall','railway_station','school','public_toilet','airport','park','other')
        AND 'accessible_toilet' = ANY(categories)));
-- Extend known proposal baselines without changing whether a proposal is current.
UPDATE marker_edit_proposals p SET base_content = p.base_content || jsonb_build_object(
    'categories', ARRAY[p.base_content->>'category'], 'opening_hours_note', NULL
) WHERE p.base_content IS NOT NULL;
