-- The activity's type tells running from cycling, so one value is enough. The
-- files are searched again, which fills it for both.
ALTER TABLE activities DROP COLUMN vo2_max_cycling;
UPDATE activities SET vo2_max_checked_at = NULL;
