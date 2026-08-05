# gh-web-dash

A local web dashboard showing recent GitHub Actions runs across all your repositories.

## Install

    cargo install --path .

## Run

    gh-web-dash

It binds a random local port, opens your browser, and starts polling GitHub. It authenticates with
`gh auth token`, falling back to `$GITHUB_TOKEN`.

Options:

- `--no-open` — do not open a browser
- `--config <path>` — use a different config file
- `--port <port>` — bind a specific port instead of an OS-assigned one
- `--host <addr>` — bind a specific address (default `127.0.0.1`, this machine only)

## Development

A devcontainer is included. It builds on `rust:latest` with `mise`, the pinned tool versions from
`mise.toml`, and the `gh` CLI. The app needs a GitHub token there just as it does on your host, so
log in once:

    just auth

That runs `gh auth login` inside the container. The credentials live in a named volume, so they
survive `just rebuild` — but a fresh clone needs it once, and `just run` will fail with "No GitHub
token found" until you do.

There is a `Justfile` wrapping the common tasks, each running inside the container:

    just auth          # gh auth login, inside the container
    just test          # cargo test
    just lint --all    # precious lint
    just tidy --all    # precious tidy
    just run           # the dashboard, on port 8420
    just shell         # a shell in the container
    just ci            # everything CI checks
    just rebuild       # recreate the container

Or directly, without a container:

    mise exec -- precious tidy --all
    mise exec -- precious lint --all

Inside a container, run the app with `--no-open`: there is no browser to launch. `just run` also
passes `--host 0.0.0.0 --port 8420`. Both are needed to reach it from outside the container: a
default `127.0.0.1` bind reaches only the container's own loopback, and the port has to be
predictable to be published. `devcontainer.json` publishes it with `appPort` — note that
`forwardPorts` alone is an editor feature and does nothing for the `devcontainer` CLI that `just`
uses. It is published to the host's loopback only, so the dashboard is not exposed to your network.

## Configuration

`~/.config/gh-web-dash/config.toml`, created with defaults on first run:

    poll_interval_secs = 180
    include_orgs = true
    ignore = ["you/old-*"]

`ignore` holds globs matched against `owner/repo`.

## What it shows

One row per repository, showing the latest run of each of its workflows. A healthy repository
collapses to a workflow count; a broken one names the workflow that broke. Rows are sorted by most
recent activity, and "Failures only" filters to repositories that are currently broken.

Clicking a row expands it to show each workflow's recent run history — newest on the left — so you
can tell a newly broken workflow from a flaky one. Every run links to github.com.

Runs on each repository's default branch, plus runs on branches you authored, are included.
Bot-authored runs are excluded. Data is cached in `~/.config/gh-web-dash/runs.db` and pruned after
30 days.

The header shows `syncing… 30/44` while a cycle is in flight, counting repositories. The tab's
favicon is green when everything passes and red when any repository is failing, so a break is
visible without switching to the tab.

## Which repositories are watched

Not every repository is polled. A repository is skipped when it matches an `ignore` glob, is
archived on GitHub, or has had no pushes in 90 days and no runs on record. That last rule keeps a
repository with only scheduled workflows visible once it has produced a run.

The **repos** link in the header opens a page listing every repository you can see, each labelled
with why it is or is not being polled, with a search box and per-category counts. From there you can
**mute** a repository that is being polled, or **include anyway** one that the rules skip — useful
for a dormant repository whose only workflow is scheduled, which no `ignore` glob could rescue,
since globs only subtract.

Those manual choices are stored in the database, not in `config.toml` — the app never rewrites your
config file.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
