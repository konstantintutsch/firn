-- Original files and images move to the file store in DATA_DIR. The backend
-- moves existing ones on start and clears these columns, which stay until a
-- later version, so that the move can be repeated after an interruption.
ALTER TABLE activity_images ALTER COLUMN data DROP NOT NULL;
