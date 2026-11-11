# Changelog

## Unreleased

### Changed

- Serve the frontend with Caddy instead of nginx. Sign-ins are limited to 20 at once and 10 a minute after that for each client.

## 0.2.1 - 2026-11-10

Firn now lives on GitHub.

### Changed

- Move the repository to GitHub. Docker images are built by GitHub Actions and published to the GitHub container registry as `ghcr.io/konstantintutsch/firn`. Update the image names in your deployment.
- Turn off the API documentation in the example configuration.
- Read `API_DOCS` only from the env file.
- Make translations optional for contributors.
- Shorten the README and getting started.

### Added

- Show the version and a link to the repository in Settings.

### Fixed

- Keep navigation labels inside the bar on narrow screens.
- Make the Authelia test password hashes in the development setup match `firn`.
- Make the About section in Settings readable.

## 0.2.0 - 2026-11-09

Runmap is now Firn.

### Changed

- Rename the project from Runmap to Firn: crate, binary, containers, images, database name and user, cookies, request header, OIDC client ID and export format. Exports made as runmap-export still import. Existing deployments need the database, its user and the Docker volumes renamed, and everyone signs in again.
- Read the Garmin Connect Personal API key only from Settings. The `GARMIN_PERSONAL_API_KEY` variable is no longer read.
- Getting started describes running Firn on a server. The local setup moved to development.
- Docker images are no longer tagged with their hash, and `latest` is tagged only for stable versions.

### Added

- License and contributing guidelines.

## 0.1.0 - 2026-11-08

First release.

### Features

- Heatmap of all runs on a global map, with smooth lines, sharp circles and one heat color along shared paths.
- Sign in with OIDC. Each account has its own data.
- Sync the full Garmin Connect copy through the Garmin Connect Personal API, by button or on a schedule, with progress, cancelling and retries after server errors and short rate limits.
- Import Apple Health exports, FIT, GPX and TCX files and earlier exports. An import can replace everything else in the account.
- Export all data as a ZIP. Download activity files as recorded or as GPX.
- Activity details with splits, colored heart rate zones, workouts, photos, weather, devices, training effect and VO2 max. Step through activities.
- Activity list with search, filters, sorting, grouping by month and endless scrolling. Filter by training block.
- Statistics with streaks, a calendar, personal records, race results and targets, a year over year comparison, a 7-day resting heart rate, a 7-day HRV and VO2 max charted with the runs.
- Routes: save, import, edit and delete them, show them on the map and as previews.
- Map previews of activities and routes.
- Search places and activities.
- Settings for units, language (English and German), theme and account deletion.
- Liquid glass interface with light mode, an app icon, keyboard use, offline use and phone support.
- Demo with made-up data from a stand-in Garmin API.
- Docker images built in CI, with example deployments and a local Authelia.

### Removed

- Hydration.
