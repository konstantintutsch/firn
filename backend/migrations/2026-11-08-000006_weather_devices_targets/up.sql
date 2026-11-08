-- The time that a runner aimed for in a race.
ALTER TABLE race_results ADD COLUMN target_time_s DOUBLE PRECISION;

-- The sensors and the watch that recorded an activity, read from its FIT file.
-- Set to the time the file was searched, like VO2 max, for the files that were
-- saved before it was read.
ALTER TABLE activities
    ADD COLUMN devices JSONB,
    ADD COLUMN devices_checked_at TIMESTAMPTZ;

-- What Garmin says about an activity, kept in columns of their own. They follow
-- the summary and the details, so a sync fills them without extra code.
ALTER TABLE activities
    ADD COLUMN weather JSONB
        GENERATED ALWAYS AS (COALESCE(details -> 'weather', summary -> 'weather')) STORED,
    ADD COLUMN training_effect_label TEXT
        GENERATED ALWAYS AS (summary ->> 'trainingEffectLabel') STORED,
    ADD COLUMN aerobic_training_effect DOUBLE PRECISION
        GENERATED ALWAYS AS (
            CASE WHEN jsonb_typeof(summary -> 'aerobicTrainingEffect') = 'number'
                THEN (summary ->> 'aerobicTrainingEffect')::double precision END
        ) STORED;
