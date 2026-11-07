-- Set once the activity's photos were handled, so an interrupted sync can resume.
ALTER TABLE activities ADD COLUMN images_synced_at TIMESTAMPTZ;

-- The photos that people added to an activity on Garmin Connect. Garmin's links
-- expire after a day, so the images themselves are kept.
CREATE TABLE activity_images (
    activity_id BIGINT NOT NULL REFERENCES activities (id) ON DELETE CASCADE,
    -- Garmin's ID of the image.
    image_id TEXT NOT NULL,
    -- The order in which Garmin lists the images.
    seq INTEGER NOT NULL,
    content_type TEXT NOT NULL,
    data BYTEA NOT NULL,
    PRIMARY KEY (activity_id, image_id)
);
