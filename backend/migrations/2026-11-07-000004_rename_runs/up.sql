-- The table holds all activity types with GPS, not only runs.
ALTER TABLE runs RENAME TO activities;
ALTER TABLE activities RENAME CONSTRAINT runs_pkey TO activities_pkey;
ALTER INDEX runs_unsynced_idx RENAME TO activities_unsynced_idx;

ALTER TABLE track_points RENAME COLUMN run_id TO activity_id;
ALTER TABLE track_points RENAME CONSTRAINT track_points_run_id_fkey TO track_points_activity_id_fkey;
