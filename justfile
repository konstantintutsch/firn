# List all commands
default:
    @just --list

# Build and start the app at http://localhost:8080
up:
    docker compose up --build --detach --wait

# Start the app and rebuild on file changes
dev:
    docker compose up --build --watch

# Stop the app and keep saved activities
down:
    docker compose down

# Follow the logs
logs *services:
    docker compose logs --follow {{services}}

# Run the backend tests in a throwaway database
test *args:
    docker compose --profile dev up --detach --wait test-db
    docker compose run --build --rm tools cargo test {{args}}
    docker compose rm --stop --force test-db

# Format the backend code
fmt:
    docker compose run --build --rm --no-deps tools cargo fmt

# Check the backend formatting and lints
lint:
    docker compose run --build --rm --no-deps tools cargo fmt --check
    docker compose run --build --rm --no-deps tools cargo clippy --all-targets -- --deny warnings

# Create a new migration
migration name:
    docker compose run --build --rm --no-deps tools diesel migration generate {{name}}

# Apply migrations to the database and regenerate src/schema.rs
schema:
    docker compose run --build --rm tools sh -c 'diesel migration run && diesel print-schema > src/schema.rs'

# Run npm in the frontend, for example `just npm install leaflet`
npm *args:
    docker compose run --rm node npm {{args}}

# Open a SQL shell on the database
psql:
    docker compose exec db psql -U runmap runmap
