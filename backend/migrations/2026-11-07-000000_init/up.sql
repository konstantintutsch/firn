CREATE TABLE runs (
    id BIGINT PRIMARY KEY,
    name TEXT NOT NULL,
    activity_type TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    distance_m DOUBLE PRECISION,
    duration_s DOUBLE PRECISION,
    -- The activity exactly as Garmin's list endpoint returned it.
    summary JSONB NOT NULL,
    -- The GPX file exactly as Garmin exported it. NULL for runs without GPS.
    gpx TEXT,
    -- Set once the track was handled, so an interrupted sync can resume.
    track_synced_at TIMESTAMPTZ
);

CREATE INDEX runs_unsynced_idx ON runs (id) WHERE track_synced_at IS NULL;

CREATE TABLE track_points (
    run_id BIGINT NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    lat DOUBLE PRECISION NOT NULL CHECK (lat BETWEEN -90 AND 90),
    lon DOUBLE PRECISION NOT NULL CHECK (lon BETWEEN -180 AND 180),
    -- Web Mercator position in the unit square, which the heatmap grid bins on.
    x DOUBLE PRECISION GENERATED ALWAYS AS ((lon + 180) / 360) STORED,
    y DOUBLE PRECISION GENERATED ALWAYS AS (
        (1 - ln(tan(radians(45 + LEAST(GREATEST(lat, -85.05), 85.05) / 2))) / pi()) / 2
    ) STORED,
    PRIMARY KEY (run_id, seq)
);

CREATE INDEX track_points_position_idx ON track_points (lon, lat);

-- Holds at most one row: the Garmin session of the single account.
CREATE TABLE garmin_tokens (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    client_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
