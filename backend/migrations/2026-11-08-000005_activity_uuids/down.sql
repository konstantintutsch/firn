ALTER TABLE activities DROP COLUMN preview;

ALTER TABLE track_points ADD COLUMN old_activity_id BIGINT;
UPDATE track_points SET old_activity_id = activities.garmin_id
    FROM activities WHERE activities.id = track_points.activity_id;
ALTER TABLE track_points DROP COLUMN activity_id;
ALTER TABLE track_points RENAME COLUMN old_activity_id TO activity_id;
ALTER TABLE track_points ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE activity_images ADD COLUMN old_activity_id BIGINT;
UPDATE activity_images SET old_activity_id = activities.garmin_id
    FROM activities WHERE activities.id = activity_images.activity_id;
ALTER TABLE activity_images DROP COLUMN activity_id;
ALTER TABLE activity_images RENAME COLUMN old_activity_id TO activity_id;
ALTER TABLE activity_images ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE race_results ADD COLUMN old_activity_id BIGINT;
UPDATE race_results SET old_activity_id = activities.garmin_id
    FROM activities WHERE activities.id = race_results.activity_id;
ALTER TABLE race_results DROP COLUMN activity_id;
ALTER TABLE race_results RENAME COLUMN old_activity_id TO activity_id;
ALTER TABLE race_results ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE routes ADD COLUMN old_activity_id BIGINT;
UPDATE routes SET old_activity_id = activities.garmin_id
    FROM activities WHERE activities.id = routes.activity_id;
ALTER TABLE routes DROP COLUMN activity_id;
ALTER TABLE routes RENAME COLUMN old_activity_id TO activity_id;

ALTER TABLE activities DROP CONSTRAINT activities_pkey;
ALTER TABLE activities DROP COLUMN id;
ALTER TABLE activities DROP CONSTRAINT activities_garmin_id_key;
ALTER TABLE activities RENAME COLUMN garmin_id TO id;
ALTER TABLE activities ADD CONSTRAINT activities_pkey PRIMARY KEY (id);
CREATE INDEX activities_unsynced_idx ON activities (id) WHERE track_synced_at IS NULL;

ALTER TABLE track_points
    ADD PRIMARY KEY (activity_id, seq),
    ADD CONSTRAINT track_points_activity_id_fkey
        FOREIGN KEY (activity_id) REFERENCES activities (id) ON DELETE CASCADE;
ALTER TABLE activity_images
    ADD PRIMARY KEY (activity_id, image_id),
    ADD CONSTRAINT activity_images_activity_id_fkey
        FOREIGN KEY (activity_id) REFERENCES activities (id) ON DELETE CASCADE;
ALTER TABLE race_results
    ADD PRIMARY KEY (activity_id),
    ADD CONSTRAINT race_results_activity_id_fkey
        FOREIGN KEY (activity_id) REFERENCES activities (id) ON DELETE CASCADE;
ALTER TABLE routes
    ADD CONSTRAINT routes_activity_id_fkey
        FOREIGN KEY (activity_id) REFERENCES activities (id) ON DELETE SET NULL;
