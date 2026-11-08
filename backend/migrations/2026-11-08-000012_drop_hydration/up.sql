-- Hydration is no longer saved or shown, from Garmin or from Apple Health.
DELETE FROM daily_metrics WHERE metric IN ('hydration_l', 'hydration_goal_l');
DELETE FROM garmin_documents WHERE kind = 'hydration';
