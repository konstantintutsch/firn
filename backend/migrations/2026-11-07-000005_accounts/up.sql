-- One row per person who signed in through OpenID Connect.
CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    oidc_issuer TEXT NOT NULL,
    oidc_subject TEXT NOT NULL,
    email TEXT,
    name TEXT,
    -- The Garmin Personal API key, encrypted with APP_SECRET: a nonce, then the ciphertext.
    garmin_api_key BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (oidc_issuer, oidc_subject)
);

-- The cookie holds a random token. Only its SHA-256 hash is stored, so that a
-- leaked database cannot be used to sign in.
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX sessions_user_idx ON sessions (user_id);

-- NULL for activities saved before accounts existed. The first account claims them.
ALTER TABLE activities
    ADD COLUMN user_id BIGINT REFERENCES users (id) ON DELETE CASCADE,
    -- The activity exactly as Garmin's activity endpoint returned it, with laps,
    -- heart rate and training effect that the list leaves out.
    ADD COLUMN details JSONB;

CREATE INDEX activities_user_idx ON activities (user_id, started_at);

-- Every Garmin response exactly as returned, so that nothing is lost when the
-- statistics later read more fields from it.
CREATE TABLE garmin_documents (
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- What was fetched, like sleep or hrv_summaries.
    kind TEXT NOT NULL,
    -- The day, the range like 2026-01-01/2026-01-28, or empty for settings.
    period TEXT NOT NULL,
    -- NULL if Garmin had nothing for the period.
    data JSONB,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, kind, period)
);

-- Daily values read from garmin_documents, for the statistics page.
CREATE TABLE daily_metrics (
    user_id BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    day DATE NOT NULL,
    metric TEXT NOT NULL,
    value DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (user_id, metric, day)
);
