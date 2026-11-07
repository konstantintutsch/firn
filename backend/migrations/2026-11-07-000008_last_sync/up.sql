-- The status of the account's last finished sync, as GET /api/sync answers it,
-- so that it survives a restart.
ALTER TABLE users ADD COLUMN last_sync JSONB;
