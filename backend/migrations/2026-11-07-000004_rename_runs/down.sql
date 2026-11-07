ALTER TABLE track_points RENAME CONSTRAINT track_points_activity_id_fkey TO track_points_run_id_fkey;
ALTER TABLE track_points RENAME COLUMN activity_id TO run_id;

ALTER INDEX activities_unsynced_idx RENAME TO runs_unsynced_idx;
ALTER TABLE activities RENAME CONSTRAINT activities_pkey TO runs_pkey;
ALTER TABLE activities RENAME TO runs;
