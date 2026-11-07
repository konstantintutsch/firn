DROP TABLE daily_metrics;
DROP TABLE garmin_documents;
DROP INDEX activities_user_idx;
ALTER TABLE activities DROP COLUMN details, DROP COLUMN user_id;
DROP TABLE sessions;
DROP TABLE users;
