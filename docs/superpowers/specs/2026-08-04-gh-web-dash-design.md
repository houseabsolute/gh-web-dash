# gh-web-dash — Design

**Date:** 2026-08-04
**Status:** Approved

## Purpose

A local web dashboard showing recent GitHub Actions runs across all of the user's
repositories (~100). Answers two questions: "what has CI been doing lately?" and
"what is broken right now?"

Runs locally, on demand. No deployment, no shared state, no other users.

## Form

A single Rust binary, `gh-web-dash`. Running it:

1. Binds `127.0.0.1:0` — the OS assigns an ephemeral port, read back from the
   listener after bind, so there is no port-in-use failure mode and no race.
2. Opens `http://127.0.0.1:<port>` in the default browser (the `open` crate).
   `--no-open` suppresses this for development loops.
3. Serves one page and polls GitHub in the background until killed.

## Scope of repositories

Auto-discovered: repositories owned by the authenticated user, plus repositories
in organizations they belong to (`include_orgs`, default true). An ignore-list of
globs matched against `owner/repo` removes noisy or archived repositories.

Auto-discovery means new repositories appear without config changes; the
ignore-list handles the ones that should not.

## Scope of runs

The feed shows runs on each repository's default branch, plus runs on branches
authored by the user. Bot-authored runs are excluded: `actor.type == "Bot"`, plus
a check for an `[bot]` suffix on the actor login.

Rationale: across ~100 repositories, dependabot and scheduled runs would dominate
a raw feed. Filtering happens during sync, before storage.

## Views

Primary view is a time-ordered feed, newest first, one line per run: status dot,
`owner/repo`, workflow name, branch, relative time. Dense — the goal is maximum
runs visible per screen.

A "Failures only" toggle filters to failed runs. Additional chips filter by
workflow name, generated from the workflow names present in the current result
set.

Clicking a row opens the run on github.com in a new tab. There is no inline
drill-down in v1; reading logs and re-running jobs both require github.com
anyway.

## Architecture

Five modules, each independently testable:

- **`config`** — parses `~/.config/gh-web-dash/config.toml`. Pure, aside from the
  file read.
- **`auth`** — resolves a token: `gh auth token` via subprocess, falling back to
  `$GITHUB_TOKEN`. If neither yields a token, errors naming both options. The
  subprocess call is injected so the resolution logic is testable.
- **`github`** — API client. Lists repositories; fetches recent workflow runs per
  repository. Owns ETags, 304 handling, and rate-limit backoff. Takes an HTTP
  client as a dependency.
- **`store`** — SQLite via `rusqlite`. Owns the schema and every query. No other
  module writes SQL.
- **`server`** — axum routes, HTML rendering, and the background poll loop.

**Data flow:** poller → `github` → `store`; browser → `store`. The request path
never touches the GitHub API. This keeps page loads fast and makes a GitHub
outage a staleness problem rather than a broken dashboard.

## Data model

```sql
repos(id, full_name, default_branch, etag, last_synced, ignored)
runs(id, repo_id, workflow_name, branch, actor, status, conclusion,
     commit_sha, commit_subject, html_url, started_at, updated_at)
```

`runs.id` is GitHub's run ID, so upserts are idempotent: re-fetching a run that
has moved from `in_progress` to `completed` updates the row rather than
duplicating it.

`repos.ignored` is a materialized flag, recomputed from the config globs on every
discovery pass — the config file remains the single source of truth, and the
column exists so queries can filter without re-evaluating globs.

The cache survives restarts, giving an instant first paint and letting the poller
run on a schedule independent of when the page is open.

## Sync

Every `poll_interval_secs` (default 180):

1. **Repository discovery** runs hourly, not every cycle — new repositories appear
   within the hour, saving ~30 requests per cycle.
2. For each non-ignored repository:
   `GET /repos/{owner}/{repo}/actions/runs?per_page=20` with the stored ETag. A
   304 costs no rate-limit quota and skips to the next repository.
3. Filter as described under *Scope of runs*, before storage.
4. Upsert. Prune runs older than 30 days.

**Rate limiting:** the client reads `x-ratelimit-remaining`. Below 500, the poll
interval doubles rather than failing. With ETags across ~100 repositories this
should not fire, but an unbounded poller sharing the user's quota could otherwise
break their `gh` CLI.

**Failure isolation:** a repository that errors (deleted, permissions changed,
transient 5xx) is logged and counted; the cycle continues. One bad repository
never aborts a cycle.

## HTTP surface

- `GET /` — page shell: header, filter chips, empty table. Server-rendered HTML
  with one embedded CSS file and one embedded JS file. No build step, no npm.
- `GET /api/runs?failures_only=&workflow=&repo=` — JSON, newest first, capped at
  200 rows. Reads SQLite only.
- `POST /api/sync` — triggers an immediate sync, returning immediately. Guarded
  so overlapping syncs cannot stack.
- `GET /api/status` — last sync time, rate-limit remaining, error count.

## Browser behavior

The page fetches `/api/runs` every 15 seconds and re-renders the table body. No
SSE, no websockets. Chip filtering is client-side over the rows already loaded,
so it is instant.

The header shows last-sync time, a **Refresh now** button hitting `/api/sync`,
and a rate-limit indicator that appears only when degraded.

**Staleness is visible.** If the last successful sync is older than three poll
intervals, the header turns amber and says so. A green board silently showing
hour-old data is worse than one that is obviously broken.

A failed `/api/runs` poll leaves the last-good table on screen and marks the
header stale. The page never blanks on a transient error.

## Configuration

`~/.config/gh-web-dash/config.toml`, created with commented defaults on first run
if absent:

```toml
poll_interval_secs = 180
include_orgs = true
ignore = ["autarch/old-*", "autarch/scratch"]
```

Nothing else is configurable in v1: the port is ephemeral, the token comes from
`gh` or the environment, retention is fixed at 30 days. No credential is ever
written to the config file.

## Error handling

**Startup (fatal, exit non-zero with a message naming the fix):** no token
available; config directory unwritable; SQLite will not open.

**Sync (non-fatal):** per-repository errors are counted, logged, and exposed via
`/api/status`. A 401 is special-cased with "token expired, run `gh auth login`" —
the one failure the user can act on directly.

**Browser (non-fatal):** as described above.

## Testing

Development follows TDD.

- `config` and the bot/branch filter logic are pure functions, unit tested. The
  filter gets the most cases — it is where subtle bugs will live.
- `github` is tested against a `wiremock` server: ETag/304 handling, pagination,
  rate-limit backoff, a repository returning 404 mid-cycle.
- `store` is tested against in-memory SQLite: upsert idempotency (same run twice,
  then with a changed conclusion), pruning, query filters.
- `server` gets integration tests through axum's test harness against a seeded
  store, asserting `/api/runs` shape and filter behavior.

No browser automation. The JavaScript is small enough that its risk does not
justify a headless-Chrome dependency.

## Explicitly out of scope for v1

- Inline job/log drill-down
- Historical analytics (failure rates, flaky-test detection, trends)
- Any view organized primarily by repository rather than by time
- Re-running or cancelling workflows from the dashboard
- Multi-user or hosted deployment

## Naming note

`gh-dash` (dlvhdr/gh-dash) is an existing popular TUI for GitHub PRs and issues.
This project is named `gh-web-dash` to avoid a binary-name collision on `$PATH`.
