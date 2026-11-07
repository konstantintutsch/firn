-- Generated from the raw summary, so that saved activities need no new sync.
ALTER TABLE runs
    ADD COLUMN description TEXT GENERATED ALWAYS AS (summary ->> 'description') STORED,
    ADD COLUMN location TEXT GENERATED ALWAYS AS (summary ->> 'locationName') STORED,
    -- Local time as Garmin formats it, like 2026-03-01 08:00:00. Dates a person
    -- searches for are local, while started_at is in UTC.
    ADD COLUMN started_local TEXT GENERATED ALWAYS AS (summary ->> 'startTimeLocal') STORED,
    ADD COLUMN search_text TEXT GENERATED ALWAYS AS (
        name || ' ' || activity_type
            || ' ' || coalesce(summary ->> 'description', '')
            || ' ' || coalesce(summary ->> 'locationName', '')
            || ' ' || coalesce(summary ->> 'startTimeLocal', '')
    ) STORED;
