-- Every VO2 max that a file holds is kept with its activity, not as one value per day.
ALTER TABLE activities ADD COLUMN vo2_max DOUBLE PRECISION;
ALTER TABLE activities ADD COLUMN vo2_max_cycling DOUBLE PRECISION;
-- The files are searched again, and the daily values that came from them or from
-- Garmin's daily max metrics are dropped.
UPDATE activities SET vo2_max_checked_at = NULL;
DELETE FROM daily_metrics WHERE metric IN ('vo2_max', 'vo2_max_cycling');
DELETE FROM garmin_documents WHERE kind = 'max_metrics';
