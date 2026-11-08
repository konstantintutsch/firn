-- Routes the account saved to run again: made from an activity or imported from GPX.
CREATE TABLE routes (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    distance_m DOUBLE PRECISION NOT NULL,
    elevation_gain_m DOUBLE PRECISION,
    -- One [latitude, longitude, elevation] per point, with null for an unknown elevation.
    points JSONB NOT NULL,
    -- The activity that the route was made from. Deleting it keeps the route.
    activity_id BIGINT REFERENCES activities (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX routes_user_idx ON routes (user_id, created_at);
