# Runmap

A heatmap of all your Garmin activities with GPS on a global map, and statistics
on your training, sleep and health. Search for places and activities, and filter
the map by type, title, description, location, length, duration and date.

Several people can use one Runmap. Each signs in through OpenID Connect and adds
their own Garmin key, and sees only their own data.

## Getting started

Requires Docker, [just](https://just.systems) and an OpenID Connect provider,
like Authentik, Keycloak, Authelia or Google.

1. Create an OpenID Connect client at your provider, with the redirect URI
   `http://localhost:8080/api/auth/callback`.
2. Copy `.env.example` to `.env`. Fill in `APP_SECRET`, `OIDC_ISSUER`,
   `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET`.
3. Run `just up`, open <http://localhost:8080> and sign in.
4. Create a Personal API key on
   [Garmin Connect](https://connect.garmin.com/app/settings/personalApiToken)
   and add it under **Settings**.
5. Select **Sync** to save your data. Select it again at any time to add new data.

To serve Runmap at another address, set `PUBLIC_URL` to it and `WEB_BIND` to
`0.0.0.0`, and register `${PUBLIC_URL}/api/auth/callback` at the provider. Use
HTTPS, for example through a reverse proxy: sign-in cookies are only marked
secure if `PUBLIC_URL` starts with `https://`.

`just` lists all commands. The API documentation is at
<http://localhost:8080/api/docs>.

## Trying it with a local Authelia

`just authelia` starts the app with a local [Authelia](https://www.authelia.com)
at <https://authelia.localhost:9091>, already set up as the provider. Sign in as
`alice` or `bob`, both with the password `runmap`. `.env` still needs
`APP_SECRET`.

The first run creates a test CA and certificate in `dev/authelia/secrets`. The
browser warns about the certificate: accept it, or trust
`dev/authelia/secrets/ca.public.crt`. Chrome and Firefox find
`authelia.localhost` on their own. Other browsers may need
`127.0.0.1 authelia.localhost` in `/etc/hosts`.

Only use this to try Runmap out: the configuration's secrets are public.

## Running it on a server

The **Images** workflow in `.gitea/workflows/images.yaml` builds the images
`<owner>/runmap/backend` and `<owner>/runmap/frontend` on Gitea Actions and
pushes them to the Gitea container registry. Add the secret `REGISTRY_TOKEN`,
an access token with the `write:package` scope. The workflow file lists the
other settings.

`examples/compose.yaml` runs those images behind your reverse proxy. Copy it
with `examples/.env.example` as `.env`, fill that in and run
`docker compose up -d`.

## What a sync saves

Runmap copies everything the Garmin Personal API offers:

- Every activity, with or without GPS: the summary, the details, the original
  file and its workouts. Only activities with GPS go on the map.
- Health data from the first activity on, and at least one year back: daily
  summaries, sleep, HRV, heart rate, stress, Body Battery, steps, intensity
  minutes, floors, SpO2, respiration, hydration, weight and body composition,
  fitness age, food logs, menstrual cycle, profile settings and heart rate zones.

Each Garmin response is stored as it came, in `garmin_documents`. The statistics
page reads daily values from them. Garmin does not document these responses, so
a value that Garmin moved may be missing from the charts, but never from the
saved data.

The first sync makes one request per day for some kinds of data, which takes a
while for a long history. A sync can be interrupted: the next one continues
where it stopped, and always fetches the last few days again, since Garmin keeps
updating them.

## Upgrading from a version without accounts

The first account to sign in takes over all saved activities. If `.env` still
holds `GARMIN_PERSONAL_API_KEY`, that account also takes over the key.

## Garmin access

Runmap uses the [Garmin Connect Personal API](https://developer.garmin.com/gc-developer-program/personal/personal-api),
which is meant for personal use. Keys expire after the period you chose when
creating them. The app tells you when a key stops working: create a new one and
add it under **Settings**.

## Data and privacy

- A Garmin key gives access to your Garmin data. Runmap stores it encrypted with
  `APP_SECRET`, and never returns it. Keep `.env` private: without `APP_SECRET`,
  a copy of the database does not reveal the keys. Changing `APP_SECRET` makes
  saved keys unreadable, so everyone has to add theirs again.
- Each activity keeps the original ZIP file from Garmin in Postgres.
- Sessions last 30 days. The database stores only a hash of each session token.
- The map shows where you go, including near your home. All data requires a
  session, and each account sees only its own data.
- Map tiles come from [OpenFreeMap](https://openfreemap.org), which sees the
  areas you view.
- Place search uses [Photon](https://photon.komoot.io), which sees what you type
  in the search box, including when you search for an activity.
