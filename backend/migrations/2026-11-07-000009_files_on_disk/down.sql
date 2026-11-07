-- Images that are only in the file store have no data here. They stay listed,
-- but older versions show them as broken until they are synced again.
UPDATE activity_images SET data = '' WHERE data IS NULL;
ALTER TABLE activity_images ALTER COLUMN data SET NOT NULL;
