# Contributing

Thanks for wanting to help. Firn is a personal project that holds health and
training data, so I keep changes small and careful.

## Before you start

Open an issue before a large change, like a new feature, so we can agree on it
first. Small fixes can go straight to a pull request.

## Setup

Requires Docker and [just](https://just.systems). See
[Development](README.md#development) for the local setup, and `just` for all
commands.

## Checks

Run both before you open a pull request. CI runs the same checks.

```sh
just lint
just test
```

## Changes

- Keep a pull request to one change.
- Add or update tests for what you change. Tests run against a throwaway
  database and never touch saved activities.
- Change the database only through a migration: `just migration <name>`, then
  `just schema`. Never edit a migration that is released, add a new one.
  Migrations need a working `down.sql`.
- User-facing text is English in the code.
- Update the README if you change how Firn is set up or configured.
- Do not commit personal data, secrets, or real activity files. The demo shows
  made-up data only.

## Commit messages

Write them according to [Conventional Commits](https://www.conventionalcommits.org),
like `fix: keep the splits of an imported file`. Mark a breaking change with `!`
and a `BREAKING CHANGE:` line.

## Review and AI-written code

I review every contribution before I merge it, and I check code from other
people more thoroughly than the AI-written code I write for this project
myself. Expect questions, and requests to explain or rework parts.

You may use AI, as I do. Say so in the pull request. Read, run and understand
what you submit. Data integrity comes first, so be extra careful with imports,
exports, sync and migrations.

## License

By contributing, you agree that your changes are released under the
[license of the project](LICENSE.txt).
