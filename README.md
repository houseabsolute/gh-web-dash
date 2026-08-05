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

A full sync takes a few minutes — the header shows `syncing… 240/490` while
one is in flight, counting repositories. With `include_orgs = true` that count
covers every organization you belong to, not just your own repositories; set it
to `false`, or use `ignore`, to trim it.
