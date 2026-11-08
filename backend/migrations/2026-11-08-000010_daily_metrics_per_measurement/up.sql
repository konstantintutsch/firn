-- A day can have several values of a metric, like the VO2 max of two workouts,
-- which are told apart by their number. A value may belong to an activity.
ALTER TABLE daily_metrics
    ADD COLUMN seq INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN activity_id UUID REFERENCES activities (id) ON DELETE SET NULL;
ALTER TABLE daily_metrics DROP CONSTRAINT daily_metrics_pkey;
ALTER TABLE daily_metrics ADD PRIMARY KEY (user_id, metric, day, seq);
CREATE INDEX daily_metrics_activity_idx ON daily_metrics (activity_id)
    WHERE activity_id IS NOT NULL;
