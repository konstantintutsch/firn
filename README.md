# Runmap

A heatmap of all your Garmin runs on a global map.

## Getting started

Requires Docker and [just](https://just.systems).

1. Create a Personal API key on
   [Garmin Connect](https://connect.garmin.com/app/settings/personalApiToken).
2. Copy `.env.example` to `.env` and add the key.
3. Run `just up` and open <http://localhost:8080>.
4. Select **Sync** to save your runs. Select it again at any time to add new ones.

`just` lists all commands. The API documentation is at
<http://localhost:8080/api/docs>.

## Garmin access

Runmap uses the [Garmin Connect Personal API](https://developer.garmin.com/gc-developer-program/personal/personal-api),
which is meant for personal use. Keys expire after the period you chose when
creating them. The app tells you when a key stops working: create a new one,
update `.env` and run `just up`.

## Data and privacy

- The API key gives access to your Garmin data. Keep `.env` private. The `api`
  container holds the key to run syncs. It never returns the key or stores it
  in the database.
- Each run keeps the original ZIP file from Garmin in Postgres.
- The map shows where you run, including near your home, and the API has no
  login. The app therefore only listens on localhost, and only accepts requests
  addressed to `localhost` or `127.0.0.1`.
- Map tiles come from [OpenFreeMap](https://openfreemap.org), which sees the
  areas you view.
