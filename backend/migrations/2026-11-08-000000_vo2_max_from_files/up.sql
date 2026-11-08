-- Set once an activity's original file was searched for VO2 max, which the
-- backend does on start for the files that were saved before it read it.
ALTER TABLE activities ADD COLUMN vo2_max_checked_at TIMESTAMPTZ;
