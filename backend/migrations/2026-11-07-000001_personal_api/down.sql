ALTER TABLE runs DROP COLUMN original_zip;
ALTER TABLE runs ADD COLUMN gpx TEXT;

CREATE TABLE garmin_tokens (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    client_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
