-- Activities get a random ID of their own. Garmin's ID stays as a plain, still
-- unique, value: it is how a sync finds an activity again.
ALTER TABLE activities
    ADD COLUMN garmin_id BIGINT,
    ADD COLUMN new_id UUID NOT NULL DEFAULT gen_random_uuid();
UPDATE activities SET garmin_id = id;
ALTER TABLE activities
    ALTER COLUMN garmin_id SET NOT NULL,
    ADD CONSTRAINT activities_garmin_id_key UNIQUE (garmin_id);

-- What refers to an activity moves to the new ID first. Dropping a column drops
-- its key and foreign key.
ALTER TABLE track_points ADD COLUMN new_activity_id UUID;
UPDATE track_points SET new_activity_id = activities.new_id
    FROM activities WHERE activities.id = track_points.activity_id;
ALTER TABLE track_points DROP COLUMN activity_id;
ALTER TABLE track_points RENAME COLUMN new_activity_id TO activity_id;
ALTER TABLE track_points ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE activity_images ADD COLUMN new_activity_id UUID;
UPDATE activity_images SET new_activity_id = activities.new_id
    FROM activities WHERE activities.id = activity_images.activity_id;
ALTER TABLE activity_images DROP COLUMN activity_id;
ALTER TABLE activity_images RENAME COLUMN new_activity_id TO activity_id;
ALTER TABLE activity_images ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE race_results ADD COLUMN new_activity_id UUID;
UPDATE race_results SET new_activity_id = activities.new_id
    FROM activities WHERE activities.id = race_results.activity_id;
ALTER TABLE race_results DROP COLUMN activity_id;
ALTER TABLE race_results RENAME COLUMN new_activity_id TO activity_id;
ALTER TABLE race_results ALTER COLUMN activity_id SET NOT NULL;

ALTER TABLE routes ADD COLUMN new_activity_id UUID;
UPDATE routes SET new_activity_id = activities.new_id
    FROM activities WHERE activities.id = routes.activity_id;
ALTER TABLE routes DROP COLUMN activity_id;
ALTER TABLE routes RENAME COLUMN new_activity_id TO activity_id;

ALTER TABLE activities DROP CONSTRAINT activities_pkey;
ALTER TABLE activities DROP COLUMN id;
ALTER TABLE activities RENAME COLUMN new_id TO id;
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

-- About 64 points along the track as [latitude, longitude], to draw a small
-- picture of the activity. NULL without a track.
ALTER TABLE activities ADD COLUMN preview JSONB;
UPDATE activities SET preview = (
    SELECT jsonb_agg(jsonb_build_array(lat, lon) ORDER BY seq)
    FROM (
        SELECT lat, lon, seq, count(*) OVER () AS total
        FROM track_points WHERE activity_id = activities.id
    ) AS points
    WHERE seq % GREATEST(1, total / 64) = 0
);
