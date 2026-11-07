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

# Start the app with a local Authelia at https://authelia.localhost:9091. Sign in as alice or bob, password runmap
authelia: authelia-secrets
    docker compose -f docker-compose.yaml -f dev/authelia/docker-compose.yaml up --build --detach --wait

# Create the test CA, certificate and signing key of the local Authelia, unless they exist
[private]
authelia-secrets:
    #!/usr/bin/env sh
    set -eu
    secrets=dev/authelia/secrets
    [ -f "$secrets/jwks.key" ] && exit 0
    mkdir -p "$secrets"
    authelia="docker run --rm --user $(id -u):$(id -g) --volume $PWD/$secrets:/secrets authelia/authelia:4.39 authelia crypto"
    # Writes ca.public.crt and ca.private.pem.
    $authelia certificate rsa generate --ca --common-name "Runmap test CA" --directory /secrets
    $authelia certificate rsa generate --common-name authelia.localhost --sans authelia.localhost \
        --path.ca /secrets --directory /secrets --file.certificate tls.crt --file.private-key tls.key
    $authelia pair rsa generate --directory /secrets --file.private-key jwks.key --file.public-key jwks.pub
