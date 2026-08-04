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

A time-ordered feed of workflow runs on each repository's default branch, plus
runs on branches you authored. Bot-authored runs are excluded. Clicking a row
opens the run on github.com.

Data is cached in `~/.config/gh-web-dash/runs.db` and pruned after 30 days.
