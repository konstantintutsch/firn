-- Official results of races, entered by hand, for the personal records.
CREATE TABLE race_results (
    activity_id BIGINT PRIMARY KEY REFERENCES activities (id) ON DELETE CASCADE,
    -- The race's distance, like 21097.5 for a half marathon.
    distance_m DOUBLE PRECISION NOT NULL,
    -- The official finish time.
    time_s DOUBLE PRECISION NOT NULL,
    event TEXT,
    -- Where the official results are published.
    results_url TEXT
);
