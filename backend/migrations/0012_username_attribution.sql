-- Accounts have always accepted 255 characters. Preserve existing long usernames
-- and their complete attribution rather than truncating or renaming accounts.
ALTER TABLE map_markers
    ALTER COLUMN username TYPE varchar(255),
    ALTER COLUMN last_edited_by TYPE varchar(255);

ALTER TABLE marker_edit_proposals
    ALTER COLUMN proposer_username TYPE varchar(255),
    ALTER COLUMN reviewed_by TYPE varchar(255);

ALTER TABLE marker_image_proposals
    ALTER COLUMN proposer_username TYPE varchar(255),
    ALTER COLUMN reviewed_by TYPE varchar(255);
