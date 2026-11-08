DROP INDEX daily_metrics_activity_idx;
DELETE FROM daily_metrics WHERE seq > 0;
ALTER TABLE daily_metrics DROP CONSTRAINT daily_metrics_pkey;
ALTER TABLE daily_metrics ADD PRIMARY KEY (user_id, metric, day);
ALTER TABLE daily_metrics DROP COLUMN activity_id, DROP COLUMN seq;
