-- The Personal API authenticates with an API key, so there is no session to keep.
DROP TABLE garmin_tokens;

-- The Personal API serves each activity's original file instead of a GPX export.
ALTER TABLE runs DROP COLUMN gpx;
ALTER TABLE runs ADD COLUMN original_zip BYTEA;

-- Download every track again, so that all runs keep their original file.
UPDATE runs SET track_synced_at = NULL;
