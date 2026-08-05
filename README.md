# gh-web-dash

A local web dashboard showing recent GitHub Actions runs across all your
repositories.

## Install

    cargo install --path .

## Run

    gh-web-dash

It binds a random local port, opens your browser, and starts polling GitHub. It
authenticates with `gh auth token`, falling back to `$GITHUB_TOKEN`.

Options:

- `--no-open` — do not open a browser
- `--config <path>` — use a different config file

## Configuration

`~/.config/gh-web-dash/config.toml`, created with defaults on first run:

    poll_interval_secs = 180
    include_orgs = true
    ignore = ["you/old-*"]

`ignore` holds globs matched against `owner/repo`.

## What it shows

One row per repository, showing the latest run of each of its workflows. A
healthy repository collapses to a workflow count; a broken one names the
workflow that broke. Rows are sorted by most recent activity, and "Failures
only" filters to repositories that are currently broken.

Clicking a row expands it to show each workflow's recent run history — newest
on the left — so you can tell a newly broken workflow from a flaky one. Every
run links to github.com.

Runs on each repository's default branch, plus runs on branches you authored,
are included. Bot-authored runs are excluded. Data is cached in
`~/.config/gh-web-dash/runs.db` and pruned after 30 days.

The header shows `syncing… 30/44` while a cycle is in flight, counting
repositories.

## Which repositories are watched

Not every repository is polled. A repository is skipped when it matches an
`ignore` glob, is archived on GitHub, or has had no pushes in 90 days and no
runs on record. That last rule keeps a repository with only scheduled workflows
visible once it has produced a run.

The **repos** link in the header opens a page listing every repository you can
see, each labelled with why it is or is not being polled, with a search box and
per-category counts. From there you can **mute** a repository that is being
polled, or **include anyway** one that the rules skip — useful for a dormant
repository whose only workflow is scheduled, which no `ignore` glob could
rescue, since globs only subtract.

Those manual choices are stored in the database, not in `config.toml` — the app
never rewrites your config file.
