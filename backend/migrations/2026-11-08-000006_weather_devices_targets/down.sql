ALTER TABLE activities
    DROP COLUMN aerobic_training_effect,
    DROP COLUMN training_effect_label,
    DROP COLUMN weather,
    DROP COLUMN devices_checked_at,
    DROP COLUMN devices;
ALTER TABLE race_results DROP COLUMN target_time_s;
