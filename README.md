# Runmap

A heatmap of all your Garmin runs on a global map.

## Getting started

Requires Docker and [just](https://just.systems).

```sh
just up      # start the app at http://localhost:8080
just login   # sign in to Garmin
just sync    # save your runs, rerun at any time to add new ones
```

`just` lists all commands. The API documentation is at
<http://localhost:8080/api/docs>.

## Garmin access

Garmin has no API for personal use, so runmap uses the unofficial API behind
Garmin's mobile apps. It can change or stop working at any time, and using it
may conflict with Garmin's terms. Garmin can also block the login. In that case
`just login` fails with an error and does not retry, because repeated attempts
can lock your account for days.

## Data and privacy

- Your password is never stored. `just login` saves a Garmin session token in
  Postgres, unencrypted. That token gives access to your Garmin account, so
  treat the database volume as a secret.
- The map shows where you run, including near your home, and the API has no
  login. The app therefore only listens on localhost.
- Map tiles come from [OpenFreeMap](https://openfreemap.org), which sees the
  areas you view.
