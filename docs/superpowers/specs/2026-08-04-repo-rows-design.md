# gh-web-dash — Repo Rows

**Date:** 2026-08-04
**Status:** Approved
**Supersedes:** the feed view described in `2026-08-04-gh-web-dash-design.md`

## Why

The chronological feed buried the question it was meant to answer. Across ~100
repositories, 200 rows are dominated by whichever repository happened to be
busy, so "which of my projects is broken?" requires scrolling past everything
that is fine.

The unit of the view changes from the run to the repository. One row per
repository, showing the state of the latest run of each of its workflows.

## What replaces what

The feed is replaced, not kept alongside. `recent_runs`,
`workflow_names_for_query`, the `/api/runs` route, and the header's workflow
chips are deleted along with their tests — dead code that still has to compile
and pass tests is a cost with no payer.

Unchanged: `config`, `auth`, `filter`, the database schema, the sync cycle
(apart from `per_page`), and the entire header — staleness, error count,
rate-limit indicator, and **Refresh now**.

## The collapsed row

One row per repository: caret, status dot, `owner/repo`, then a pill naming each
workflow whose latest run is not green, then a muted count of the remainder
("2 workflows" when all are fine; "+2 ok" beside a red pill when one is not).
Relative time on the right.

Naming only what is broken is the point. Most of ~100 repositories are green
most of the time; a row that spells out "Run tests ✓" ninety times spends its
width restating the uninteresting case. A bare dot per workflow would be denser
still, but then a red dot on a three-workflow repository forces an expansion to
learn *which* workflow broke — the most common thing the user wants to know.

**Ordering:** most recent activity first. "Failures only" remains as the way to
surface something that broke days ago and has since sunk down the list.

## The expansion

Clicking anywhere on a row toggles it open. Inside, one block per workflow:

- the workflow name
- a history strip of up to ~10 recent runs, **newest on the left**
- a muted line with the latest run's branch, relative time, and commit subject

The strip is what makes history worth showing. A red dot says a workflow is
broken; `▮▮▯▯▯▮▯▯` says it is *flaky*, which is a different problem with a
different fix.

Every strip segment links to that run on github.com, as does the detail line.
Clicking a link must not collapse the row.

Three behaviors carry more weight than they appear to:

- **Expansion survives the 15-second refresh.** The set of expanded repositories
  lives in browser state and is restored on every re-render. A row that
  collapses under the user every 15 seconds makes the feature unusable. This is
  the most likely thing to get wrong.
- **Multiple rows open at once.** No accordion — comparing two flaky
  repositories is a real need.
- **History loads lazily.** The poll fetches `/api/repos`; strips come from
  `/api/history` when a row opens, refreshed on each poll while it stays open.
  Shipping ten runs for each of 48 workflows to a page displaying none of them
  would be waste repeated every 15 seconds.

## Data and queries

No schema change. This is a different question asked of the rows already stored.

Two new `Store` methods replace the two being deleted:

- **`repo_summaries(failures_only)`** — for every non-ignored repository with at
  least one stored run, the repository name plus, for each of its workflows, the
  latest run: status, conclusion, branch, commit subject, started-at, and URL.
  One query using `ROW_NUMBER() OVER (PARTITION BY repo_full_name,
  workflow_name ORDER BY started_at DESC)` — not a query per repository.
- **`workflow_history(repo, workflow, limit)`** — the last N runs of one
  workflow, newest first. The strip renders in that order directly: leftmost
  segment is the most recent run, matching the leading status dot beside the
  repository name.

A repository with no runs in the retained window does not appear. There is
nothing to say about it, and a hundred rows of "no data" would bury the ninety
that have some.

Derived in SQL, not in the browser:

- **Repository status** — the worst status across its workflows' latest runs,
  ordered failure > in-progress > success. Drives the leading dot and the
  `failures_only` filter.
- **Repository time** — the newest `started_at` among those latest runs. Drives
  the sort.

`failures_only` now means "repositories whose worst workflow status is a
failure" rather than "runs that failed" — the same chip against a different
unit, because the row is a different thing.

**Poller change:** `per_page` goes from 20 to 50 in `github.rs`, so strips have
more to draw on. Same number of requests, same ETag behavior, warm cycles still
cost no quota. Strips show what is known rather than padding to a fixed width —
a six-segment strip is honest.

## HTTP surface

- `GET /api/repos?failures_only=` — repositories with their per-workflow latest
  runs, sorted newest activity first. Replaces `/api/runs`.
- `GET /api/history?repo=owner/name` — per-workflow run history for one
  repository, for the strips.
- `GET /api/status`, `POST /api/sync` — unchanged.
- `GET /`, `/app.css`, `/app.js`, `/render.js` — the page.

## Code organization

Two files are near the point where they do too much to hold in one view:

- `store.rs` is 575 lines and would gain two window-function queries plus their
  tests. Its tests move to `tests/store.rs`; `Store`'s API is already fully
  public and the tests use nothing private, so this is a move rather than a
  rewrite, leaving ~275 lines of implementation.
- `app.js` is 155 lines and would roughly double, since expansion state and lazy
  history are most of the new logic. It splits: `app.js` keeps polling, state,
  and the header; `render.js` takes row and expansion rendering. Two
  `include_str!`s, two routes, still no build step and no npm.

## Testing

`repo_summaries` gets store tests for what a carelessly written window function
gets wrong:

- a repository whose workflows have different latest runs
- a repository where the worst status comes from the *older* of two workflows
- in-progress ranking above success but below failure
- a repository whose only runs fall outside the retention window

`workflow_history` gets ordering and limit tests. `tests/api.rs` covers
`/api/repos` shape, sort order, and `failures_only`, plus `/api/history` for a
repository with two workflows.

The expansion behaviors — surviving refresh, multiple open, links not
collapsing — are JavaScript with no test harness. They are verified by driving
the real page in a browser, not asserted from the code.

## Out of scope

Unchanged from the original spec: no inline job or log drill-down, no historical
analytics, no re-running or cancelling workflows, no multi-user deployment. Also
not in this change: repository name search, and any second view — the feed is
gone, not hidden behind a toggle.
