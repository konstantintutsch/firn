-- The workouts that Garmin associates with an activity, as returned.
ALTER TABLE activities ADD COLUMN workouts JSONB;
