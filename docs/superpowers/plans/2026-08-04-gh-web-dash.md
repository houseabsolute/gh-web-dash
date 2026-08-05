# gh-web-dash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single Rust binary that serves a local web dashboard of recent GitHub Actions runs
across all of the user's repositories.

**Architecture:** A background poller fetches workflow runs from the GitHub API and writes them to
SQLite. An axum HTTP server reads only from SQLite and serves one dense, time-ordered feed page. The
request path never touches GitHub, so page loads are fast and a GitHub outage becomes a staleness
problem rather than a broken dashboard.

**Tech Stack:** Rust, tokio, axum 0.8, reqwest, rusqlite (bundled SQLite), serde, chrono, globset,
wiremock (tests).

**Spec:** `docs/superpowers/specs/2026-08-04-gh-web-dash-design.md`

---

## File Structure

| File                    | Responsibility                                                         |
| ----------------------- | ---------------------------------------------------------------------- |
| `Cargo.toml`            | Crate metadata and dependencies                                        |
| `src/main.rs`           | CLI args, wiring, ephemeral port bind, browser launch, poll loop spawn |
| `src/config.rs`         | Parse `~/.config/gh-web-dash/config.toml`; ignore-glob matching        |
| `src/auth.rs`           | Resolve a GitHub token from `gh auth token` or `$GITHUB_TOKEN`         |
| `src/filter.rs`         | Pure predicate deciding which runs are kept                            |
| `src/github.rs`         | GitHub REST client: repos, runs, ETags, rate-limit reporting           |
| `src/store.rs`          | SQLite schema and every query                                          |
| `src/server.rs`         | axum routes and handlers                                               |
| `src/sync.rs`           | The sync cycle: discovery, fetch, filter, upsert, prune                |
| `src/assets/index.html` | Page shell (embedded at compile time)                                  |
| `src/assets/app.css`    | Styles (embedded)                                                      |
| `src/assets/app.js`     | Polling, rendering, chip filtering (embedded)                          |
| `tests/api.rs`          | Integration tests for the HTTP surface against a seeded store          |

Each module is testable without the others: `config`, `auth`, and `filter` are pure logic; `github`
takes a base URL so it can point at a mock server; `store` opens in-memory SQLite; `server` takes a
`Store`.

---

### Task 1: Project scaffold

**Files:**

- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore` (already exists — verify contents)

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "gh-web-dash"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
axum = "0.8"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
dirs = "5"
globset = "0.4"
open = "5"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "time", "sync", "process"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
http-body-util = "0.1"
tower = { version = "0.5", features = ["util"] }
wiremock = "0.6"
```

- [ ] **Step 2: Create a placeholder `src/main.rs`**

```rust
mod auth;
mod config;
mod filter;
mod github;
mod server;
mod store;
mod sync;

fn main() {
    println!("gh-web-dash");
}
```

Create empty `src/auth.rs`, `src/config.rs`, `src/filter.rs`, `src/github.rs`, `src/server.rs`,
`src/store.rs`, `src/sync.rs` so the module declarations resolve. Each may be a single blank line
for now.

- [ ] **Step 3: Verify it builds**

Run: `cargo build` Expected: compiles with warnings about unused modules, no errors.

- [ ] **Step 4: Verify `.gitignore` contains `/target`**

Run: `cat .gitignore` Expected: includes `/target`, `/.superpowers/`, `*.db`. Add any that are
missing.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src .gitignore
git commit -m "chore: scaffold gh-web-dash crate"
```

---

### Task 2: Config parsing and ignore globs

**Files:**

- Modify: `src/config.rs`

`Config` is what the user's TOML deserializes into. `ignore` holds glob patterns matched against
`owner/repo` strings. Globs are compiled once into a `globset::GlobSet` because matching happens for
every repository on every discovery pass.

- [ ] **Step 1: Write the failing tests**

Append to `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_empty_config() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.poll_interval_secs, 180);
        assert!(c.include_orgs);
        assert!(c.ignore.is_empty());
    }

    #[test]
    fn parses_all_fields() {
        let c = Config::from_toml(
            r#"
poll_interval_secs = 60
include_orgs = false
ignore = ["autarch/old-*"]
"#,
        )
        .unwrap();
        assert_eq!(c.poll_interval_secs, 60);
        assert!(!c.include_orgs);
        assert_eq!(c.ignore, vec!["autarch/old-*".to_string()]);
    }

    #[test]
    fn rejects_zero_poll_interval() {
        let err = Config::from_toml("poll_interval_secs = 0").unwrap_err();
        assert!(err.to_string().contains("poll_interval_secs"), "got: {err}");
    }

    #[test]
    fn ignore_matcher_matches_globs_and_exact_names() {
        let c = Config::from_toml(r#"ignore = ["autarch/old-*", "autarch/scratch"]"#).unwrap();
        let m = c.ignore_matcher().unwrap();
        assert!(m.is_ignored("autarch/old-thing"));
        assert!(m.is_ignored("autarch/scratch"));
        assert!(!m.is_ignored("autarch/precious"));
        // A glob must not match across the slash separator.
        assert!(!m.is_ignored("other/old-thing"));
    }

    #[test]
    fn empty_ignore_list_matches_nothing() {
        let c = Config::from_toml("").unwrap();
        let m = c.ignore_matcher().unwrap();
        assert!(!m.is_ignored("autarch/anything"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config::` Expected: FAIL — `Config` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/config.rs`:

```rust
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = r#"# gh-web-dash configuration

# How often to poll GitHub for new workflow runs, in seconds.
poll_interval_secs = 180

# Include repositories from organizations you belong to, not just your own.
include_orgs = true

# Repositories to skip, as globs matched against "owner/repo".
ignore = []
"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_include_orgs")]
    pub include_orgs: bool,
    #[serde(default)]
    pub ignore: Vec<String>,
}

fn default_poll_interval() -> u64 {
    180
}

fn default_include_orgs() -> bool {
    true
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config> {
        let c: Config = toml::from_str(s).context("failed to parse config file")?;
        if c.poll_interval_secs == 0 {
            bail!("poll_interval_secs must be greater than 0");
        }
        Ok(c)
    }

    /// Load the config, creating it with commented defaults if absent.
    pub fn load_or_create(path: &Path) -> Result<Config> {
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("cannot create config directory {}", dir.display()))?;
            }
            std::fs::write(path, DEFAULT_CONFIG)
                .with_context(|| format!("cannot write config file {}", path.display()))?;
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config file {}", path.display()))?;
        Config::from_toml(&text)
    }

    pub fn ignore_matcher(&self) -> Result<IgnoreMatcher> {
        let mut b = GlobSetBuilder::new();
        for pat in &self.ignore {
            let glob = Glob::new(pat).with_context(|| format!("invalid ignore glob: {pat}"))?;
            b.add(glob);
        }
        Ok(IgnoreMatcher {
            set: b.build().context("failed to build ignore matcher")?,
        })
    }
}

pub struct IgnoreMatcher {
    set: GlobSet,
}

impl IgnoreMatcher {
    pub fn is_ignored(&self, full_name: &str) -> bool {
        self.set.is_match(full_name)
    }
}

/// `~/.config/gh-web-dash/config.toml`
pub fn default_config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("cannot determine your config directory")?;
    Ok(dir.join("gh-web-dash").join("config.toml"))
}

/// `~/.config/gh-web-dash/runs.db`
pub fn default_db_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("cannot determine your config directory")?;
    Ok(dir.join("gh-web-dash").join("runs.db"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config::` Expected: 5 tests pass.

Note: `globset` treats `*` as not matching `/` by default, which is what makes the `other/old-thing`
assertion pass. If it fails, the glob needs `Glob::new(pat)` with default options — do not enable
`literal_separator(false)`.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: config parsing with ignore globs"
```

---

### Task 3: Token resolution

**Files:**

- Modify: `src/auth.rs`

The decision logic (prefer `gh`, fall back to the environment, error naming both) is separated from
running the subprocess so it can be tested without a `gh` binary present.

- [ ] **Step 1: Write the failing tests**

Append to `src/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_gh_token() {
        let t = choose_token(Some("gh-token".into()), Some("env-token".into())).unwrap();
        assert_eq!(t, "gh-token");
    }

    #[test]
    fn falls_back_to_env() {
        let t = choose_token(None, Some("env-token".into())).unwrap();
        assert_eq!(t, "env-token");
    }

    #[test]
    fn blank_gh_output_is_not_a_token() {
        let t = choose_token(Some("  \n".into()), Some("env-token".into())).unwrap();
        assert_eq!(t, "env-token");
    }

    #[test]
    fn trims_whitespace() {
        let t = choose_token(Some("  gh-token\n".into()), None).unwrap();
        assert_eq!(t, "gh-token");
    }

    #[test]
    fn error_mentions_both_options() {
        let err = choose_token(None, None).unwrap_err().to_string();
        assert!(err.contains("gh auth login"), "got: {err}");
        assert!(err.contains("GITHUB_TOKEN"), "got: {err}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test auth::` Expected: FAIL — `choose_token` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/auth.rs`:

```rust
use anyhow::{bail, Result};

/// Decide which token to use. Pure — takes what the world reported.
pub fn choose_token(gh_output: Option<String>, env_token: Option<String>) -> Result<String> {
    for candidate in [gh_output, env_token] {
        if let Some(s) = candidate {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }
    bail!(
        "No GitHub token found. Either run `gh auth login`, or set GITHUB_TOKEN \
         in your environment."
    )
}

/// Resolve a token from the real world: `gh auth token`, then `$GITHUB_TOKEN`.
pub async fn resolve_token() -> Result<String> {
    let gh_output = match tokio::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        // `gh` missing or not logged in — not an error yet, the env var may work.
        _ => None,
    };
    choose_token(gh_output, std::env::var("GITHUB_TOKEN").ok())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test auth::` Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/auth.rs
git commit -m "feat: resolve GitHub token from gh CLI or environment"
```

---

### Task 4: Run filtering

**Files:**

- Modify: `src/filter.rs`

This is where the subtle bugs live, so it gets the most test cases. The rule: keep a run if it is on
the repository's default branch, **or** its actor is the authenticated user. Drop anything
bot-authored regardless of branch.

- [ ] **Step 1: Write the failing tests**

Append to `src/filter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cand(branch: &str, login: &str, actor_type: &str) -> RunCandidate {
        RunCandidate {
            branch: branch.to_string(),
            actor_login: login.to_string(),
            actor_type: actor_type.to_string(),
        }
    }

    #[test]
    fn keeps_default_branch_run_by_user() {
        assert!(should_keep(&cand("main", "autarch", "User"), "main", "autarch"));
    }

    #[test]
    fn keeps_default_branch_run_by_another_human() {
        assert!(should_keep(&cand("main", "someone", "User"), "main", "autarch"));
    }

    #[test]
    fn keeps_own_branch_run() {
        assert!(should_keep(&cand("fix-sort", "autarch", "User"), "main", "autarch"));
    }

    #[test]
    fn drops_other_humans_branch_run() {
        assert!(!should_keep(&cand("their-fix", "someone", "User"), "main", "autarch"));
    }

    #[test]
    fn drops_bot_run_on_a_branch() {
        assert!(!should_keep(
            &cand("dependabot/cargo/serde-1.0.2", "dependabot[bot]", "Bot"),
            "main",
            "autarch"
        ));
    }

    #[test]
    fn drops_bot_run_on_the_default_branch() {
        assert!(!should_keep(&cand("main", "dependabot[bot]", "Bot"), "main", "autarch"));
    }

    #[test]
    fn drops_bot_by_login_suffix_when_type_is_wrong() {
        // Some payloads report type "User" for apps; the login suffix is the backstop.
        assert!(!should_keep(&cand("main", "renovate[bot]", "User"), "main", "autarch"));
    }

    #[test]
    fn actor_type_check_is_case_insensitive() {
        assert!(!should_keep(&cand("main", "some-app", "bot"), "main", "autarch"));
    }

    #[test]
    fn user_comparison_is_case_insensitive() {
        assert!(should_keep(&cand("fix", "AUTARCH", "User"), "main", "autarch"));
    }

    #[test]
    fn respects_non_main_default_branch() {
        assert!(should_keep(&cand("master", "someone", "User"), "master", "autarch"));
        assert!(!should_keep(&cand("main", "someone", "User"), "master", "autarch"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test filter::` Expected: FAIL — `RunCandidate` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/filter.rs`:

```rust
/// The fields of a workflow run that decide whether it belongs in the feed.
#[derive(Debug, Clone)]
pub struct RunCandidate {
    pub branch: String,
    pub actor_login: String,
    pub actor_type: String,
}

pub fn is_bot(actor_login: &str, actor_type: &str) -> bool {
    actor_type.eq_ignore_ascii_case("bot") || actor_login.to_ascii_lowercase().ends_with("[bot]")
}

/// Keep runs on the default branch, plus runs authored by the current user.
/// Bot-authored runs are never kept.
pub fn should_keep(c: &RunCandidate, default_branch: &str, current_user: &str) -> bool {
    if is_bot(&c.actor_login, &c.actor_type) {
        return false;
    }
    c.branch == default_branch || c.actor_login.eq_ignore_ascii_case(current_user)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test filter::` Expected: 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/filter.rs
git commit -m "feat: run filtering by branch and actor"
```

---

### Task 5: SQLite store

**Files:**

- Modify: `src/store.rs`

`Store` owns the schema and every query. Because `rusqlite` is synchronous and the poll cycle is not
hot, the connection is wrapped in a `Mutex` and shared — simpler than a pool and adequate for one
user.

- [ ] **Step 1: Write the failing tests**

Append to `src/store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: i64, repo: &str, conclusion: Option<&str>, started: &str) -> StoredRun {
        StoredRun {
            id,
            repo_full_name: repo.to_string(),
            workflow_name: "test.yml".to_string(),
            branch: "main".to_string(),
            actor: "autarch".to_string(),
            status: "completed".to_string(),
            conclusion: conclusion.map(|s| s.to_string()),
            commit_sha: "abc123".to_string(),
            commit_subject: "Do a thing".to_string(),
            html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
            started_at: started.to_string(),
        }
    }

    #[test]
    fn upsert_is_idempotent_and_updates_in_place() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();

        s.upsert_run(&run(1, "autarch/precious", None, "2026-08-04T10:00:00Z")).unwrap();
        s.upsert_run(&run(1, "autarch/precious", None, "2026-08-04T10:00:00Z")).unwrap();
        assert_eq!(s.recent_runs(&RunQuery::default()).unwrap().len(), 1);

        s.upsert_run(&run(1, "autarch/precious", Some("failure"), "2026-08-04T10:00:00Z"))
            .unwrap();
        let rows = s.recent_runs(&RunQuery::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].conclusion.as_deref(), Some("failure"));
    }

    #[test]
    fn recent_runs_are_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        s.upsert_run(&run(1, "autarch/precious", Some("success"), "2026-08-04T09:00:00Z")).unwrap();
        s.upsert_run(&run(2, "autarch/precious", Some("success"), "2026-08-04T11:00:00Z")).unwrap();
        let rows = s.recent_runs(&RunQuery::default()).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn failures_only_excludes_success_and_in_progress() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        s.upsert_run(&run(1, "autarch/precious", Some("success"), "2026-08-04T09:00:00Z")).unwrap();
        s.upsert_run(&run(2, "autarch/precious", Some("failure"), "2026-08-04T10:00:00Z")).unwrap();
        s.upsert_run(&run(3, "autarch/precious", None, "2026-08-04T11:00:00Z")).unwrap();

        let q = RunQuery { failures_only: true, ..RunQuery::default() };
        let rows = s.recent_runs(&q).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn limit_caps_results() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        for i in 1..=5 {
            s.upsert_run(&run(i, "autarch/precious", Some("success"), "2026-08-04T09:00:00Z"))
                .unwrap();
        }
        let q = RunQuery { limit: 2, ..RunQuery::default() };
        assert_eq!(s.recent_runs(&q).unwrap().len(), 2);
    }

    #[test]
    fn prune_removes_only_old_runs() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        s.upsert_run(&run(1, "autarch/precious", Some("success"), "2020-01-01T00:00:00Z")).unwrap();
        s.upsert_run(&run(2, "autarch/precious", Some("success"), "2026-08-04T09:00:00Z")).unwrap();

        let removed = s.prune_before("2026-07-05T00:00:00Z").unwrap();
        assert_eq!(removed, 1);
        let rows = s.recent_runs(&RunQuery::default()).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn etag_round_trips_and_starts_empty() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        assert_eq!(s.repo_etag("autarch/precious").unwrap(), None);
        s.set_repo_etag("autarch/precious", "W/\"abc\"").unwrap();
        assert_eq!(s.repo_etag("autarch/precious").unwrap(), Some("W/\"abc\"".to_string()));
    }

    #[test]
    fn upsert_repo_updates_default_branch_without_losing_etag() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "master").unwrap();
        s.set_repo_etag("autarch/precious", "W/\"abc\"").unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        assert_eq!(s.repo_etag("autarch/precious").unwrap(), Some("W/\"abc\"".to_string()));
        assert_eq!(s.active_repos().unwrap()[0].default_branch, "main");
    }

    #[test]
    fn ignored_repos_are_excluded_from_active_repos() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        s.upsert_repo("autarch/scratch", "main").unwrap();
        s.set_repo_ignored("autarch/scratch", true).unwrap();
        let names: Vec<_> = s.active_repos().unwrap().into_iter().map(|r| r.full_name).collect();
        assert_eq!(names, vec!["autarch/precious".to_string()]);
    }

    #[test]
    fn workflow_names_are_distinct_and_sorted() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_repo("autarch/precious", "main").unwrap();
        let mut r = run(1, "autarch/precious", Some("success"), "2026-08-04T09:00:00Z");
        r.workflow_name = "release.yml".to_string();
        s.upsert_run(&r).unwrap();
        s.upsert_run(&run(2, "autarch/precious", Some("success"), "2026-08-04T10:00:00Z")).unwrap();
        assert_eq!(
            s.workflow_names().unwrap(),
            vec!["release.yml".to_string(), "test.yml".to_string()]
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test store::` Expected: FAIL — `Store` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/store.rs`:

```rust
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::Serialize;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS repos (
    full_name      TEXT PRIMARY KEY,
    default_branch TEXT NOT NULL,
    etag           TEXT,
    last_synced    TEXT,
    ignored        INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS runs (
    id             INTEGER PRIMARY KEY,
    repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
    workflow_name  TEXT NOT NULL,
    branch         TEXT NOT NULL,
    actor          TEXT NOT NULL,
    status         TEXT NOT NULL,
    conclusion     TEXT,
    commit_sha     TEXT NOT NULL,
    commit_subject TEXT NOT NULL,
    html_url       TEXT NOT NULL,
    started_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS runs_started_at ON runs(started_at DESC);
"#;

/// A workflow run as stored and served. Timestamps are RFC 3339 strings, which
/// sort correctly as text and need no conversion on the way to JSON.
#[derive(Debug, Clone, Serialize)]
pub struct StoredRun {
    pub id: i64,
    pub repo_full_name: String,
    pub workflow_name: String,
    pub branch: String,
    pub actor: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub commit_sha: String,
    pub commit_subject: String,
    pub html_url: String,
    pub started_at: String,
}

#[derive(Debug, Clone)]
pub struct StoredRepo {
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone)]
pub struct RunQuery {
    pub failures_only: bool,
    pub workflow: Option<String>,
    pub repo: Option<String>,
    pub limit: usize,
}

impl Default for RunQuery {
    fn default() -> Self {
        RunQuery { failures_only: false, workflow: None, repo: None, limit: 200 }
    }
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create data directory {}", dir.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open database {}", path.display()))?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Store> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA).context("failed to create schema")?;
        Ok(Store { conn: Arc::new(Mutex::new(conn)) })
    }

    pub fn upsert_repo(&self, full_name: &str, default_branch: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repos (full_name, default_branch) VALUES (?1, ?2)
             ON CONFLICT(full_name) DO UPDATE SET default_branch = excluded.default_branch",
            params![full_name, default_branch],
        )?;
        Ok(())
    }

    pub fn set_repo_ignored(&self, full_name: &str, ignored: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE repos SET ignored = ?2 WHERE full_name = ?1",
            params![full_name, ignored as i64],
        )?;
        Ok(())
    }

    pub fn repo_etag(&self, full_name: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let etag = conn.query_row(
            "SELECT etag FROM repos WHERE full_name = ?1",
            params![full_name],
            |row| row.get::<_, Option<String>>(0),
        )?;
        Ok(etag)
    }

    pub fn set_repo_etag(&self, full_name: &str, etag: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE repos SET etag = ?2 WHERE full_name = ?1",
            params![full_name, etag],
        )?;
        Ok(())
    }

    pub fn active_repos(&self) -> Result<Vec<StoredRepo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT full_name, default_branch FROM repos WHERE ignored = 0 ORDER BY full_name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(StoredRepo { full_name: row.get(0)?, default_branch: row.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn upsert_run(&self, r: &StoredRun) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO runs (id, repo_full_name, workflow_name, branch, actor, status,
                               conclusion, commit_sha, commit_subject, html_url, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
                 status = excluded.status,
                 conclusion = excluded.conclusion,
                 commit_subject = excluded.commit_subject,
                 started_at = excluded.started_at",
            params![
                r.id,
                r.repo_full_name,
                r.workflow_name,
                r.branch,
                r.actor,
                r.status,
                r.conclusion,
                r.commit_sha,
                r.commit_subject,
                r.html_url,
                r.started_at,
            ],
        )?;
        Ok(())
    }

    pub fn recent_runs(&self, q: &RunQuery) -> Result<Vec<StoredRun>> {
        let mut sql = String::from(
            "SELECT id, repo_full_name, workflow_name, branch, actor, status, conclusion,
                    commit_sha, commit_subject, html_url, started_at
             FROM runs WHERE 1 = 1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if q.failures_only {
            sql.push_str(" AND conclusion IN ('failure', 'timed_out', 'startup_failure')");
        }
        if let Some(w) = &q.workflow {
            args.push(Box::new(w.clone()));
            sql.push_str(&format!(" AND workflow_name = ?{}", args.len()));
        }
        if let Some(r) = &q.repo {
            args.push(Box::new(r.clone()));
            sql.push_str(&format!(" AND repo_full_name = ?{}", args.len()));
        }
        args.push(Box::new(q.limit as i64));
        sql.push_str(&format!(" ORDER BY started_at DESC, id DESC LIMIT ?{}", args.len()));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(refs.as_slice(), |row| {
                Ok(StoredRun {
                    id: row.get(0)?,
                    repo_full_name: row.get(1)?,
                    workflow_name: row.get(2)?,
                    branch: row.get(3)?,
                    actor: row.get(4)?,
                    status: row.get(5)?,
                    conclusion: row.get(6)?,
                    commit_sha: row.get(7)?,
                    commit_subject: row.get(8)?,
                    html_url: row.get(9)?,
                    started_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn workflow_names(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT DISTINCT workflow_name FROM runs ORDER BY workflow_name")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete runs started before an RFC 3339 cutoff. Returns the number removed.
    pub fn prune_before(&self, cutoff_rfc3339: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM runs WHERE started_at < ?1", params![cutoff_rfc3339])?;
        Ok(n)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test store::` Expected: 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/store.rs
git commit -m "feat: SQLite store for repos and runs"
```

---

### Task 6: GitHub API client

**Files:**

- Modify: `src/github.rs`

The client takes a base URL so tests point it at a `wiremock` server. It reports rate-limit headers
back to the caller rather than deciding policy itself — backoff belongs to the sync loop.

- [ ] **Step 1: Write the failing tests**

Append to `src/github.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> Client {
        Client::new(server.uri(), "test-token".to_string()).unwrap()
    }

    #[tokio::test]
    async fn fetches_current_user() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "login": "autarch"
            })))
            .mount(&server)
            .await;

        assert_eq!(client(&server).current_user().await.unwrap(), "autarch");
    }

    #[tokio::test]
    async fn lists_repos_across_pages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"full_name": "autarch/a", "default_branch": "main"}
            ])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let repos = client(&server).list_repos(false).await.unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].full_name, "autarch/a");
        assert_eq!(repos[0].default_branch, "main");
    }

    #[tokio::test]
    async fn fetches_runs_and_reports_etag_and_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"abc\"")
                    .insert_header("x-ratelimit-remaining", "4321")
                    .set_body_json(serde_json::json!({
                        "workflow_runs": [{
                            "id": 42,
                            "name": "test.yml",
                            "head_branch": "main",
                            "status": "completed",
                            "conclusion": "failure",
                            "head_sha": "abc123",
                            "html_url": "https://github.com/autarch/a/actions/runs/42",
                            "run_started_at": "2026-08-04T10:00:00Z",
                            "updated_at": "2026-08-04T10:05:00Z",
                            "actor": {"login": "autarch", "type": "User"},
                            "head_commit": {"message": "Fix the thing\n\nDetails here"}
                        }]
                    })),
            )
            .mount(&server)
            .await;

        let resp = client(&server).list_runs("autarch/a", None).await.unwrap();
        assert!(!resp.not_modified);
        assert_eq!(resp.etag.as_deref(), Some("W/\"abc\""));
        assert_eq!(resp.rate_limit_remaining, Some(4321));
        assert_eq!(resp.runs.len(), 1);
        let r = &resp.runs[0];
        assert_eq!(r.id, 42);
        assert_eq!(r.workflow_name, "test.yml");
        assert_eq!(r.actor.login, "autarch");
        // Only the commit subject, not the body.
        assert_eq!(r.commit_subject(), "Fix the thing");
    }

    #[tokio::test]
    async fn not_modified_yields_no_runs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .and(header("if-none-match", "W/\"abc\""))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;

        let resp = client(&server).list_runs("autarch/a", Some("W/\"abc\"")).await.unwrap();
        assert!(resp.not_modified);
        assert!(resp.runs.is_empty());
    }

    #[tokio::test]
    async fn missing_repo_is_an_error_not_a_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/gone/actions/runs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = client(&server).list_runs("autarch/gone", None).await.unwrap_err();
        assert!(matches!(err, GithubError::Status { status: 404, .. }), "got: {err:?}");
    }

    #[tokio::test]
    async fn unauthorized_is_distinguishable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = client(&server).list_runs("autarch/a", None).await.unwrap_err();
        assert!(err.is_unauthorized(), "got: {err:?}");
    }

    #[test]
    fn commit_subject_handles_missing_commit() {
        let r = Run {
            id: 1,
            workflow_name: "test.yml".into(),
            head_branch: "main".into(),
            status: "completed".into(),
            conclusion: None,
            head_sha: "abc".into(),
            html_url: "https://example.com".into(),
            run_started_at: None,
            updated_at: "2026-08-04T10:00:00Z".into(),
            actor: Actor { login: "autarch".into(), r#type: "User".into() },
            head_commit: None,
        };
        assert_eq!(r.commit_subject(), "");
        // With no run_started_at, fall back to updated_at so ordering still works.
        assert_eq!(r.started_at(), "2026-08-04T10:00:00Z");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test github::` Expected: FAIL — `Client` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/github.rs`:

```rust
use anyhow::Result;
use serde::Deserialize;

pub const GITHUB_API: &str = "https://api.github.com";
const USER_AGENT: &str = "gh-web-dash";
const RUNS_PER_REPO: usize = 20;
const REPOS_PER_PAGE: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("HTTP {status} from {url}")]
    Status { status: u16, url: String },
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

impl GithubError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, GithubError::Status { status: 401, .. })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
    pub r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub id: i64,
    #[serde(rename = "name", default)]
    pub workflow_name: String,
    #[serde(default)]
    pub head_branch: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(default)]
    pub head_sha: String,
    pub html_url: String,
    pub run_started_at: Option<String>,
    pub updated_at: String,
    pub actor: Actor,
    pub head_commit: Option<Commit>,
}

impl Run {
    /// The first line of the commit message.
    pub fn commit_subject(&self) -> String {
        self.head_commit
            .as_ref()
            .and_then(|c| c.message.lines().next())
            .unwrap_or("")
            .to_string()
    }

    /// When the run started, falling back to `updated_at` if GitHub omits it.
    pub fn started_at(&self) -> String {
        self.run_started_at.clone().unwrap_or_else(|| self.updated_at.clone())
    }
}

#[derive(Debug, Deserialize)]
struct RunsBody {
    #[serde(default)]
    workflow_runs: Vec<Run>,
}

#[derive(Debug)]
pub struct RunsResponse {
    pub runs: Vec<Run>,
    pub etag: Option<String>,
    pub not_modified: bool,
    pub rate_limit_remaining: Option<i64>,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl Client {
    pub fn new(base_url: String, token: String) -> Result<Client> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Client { http, base_url: base_url.trim_end_matches('/').to_string(), token })
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
    }

    pub async fn current_user(&self) -> Result<String, GithubError> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let url = format!("{}/user", self.base_url);
        let resp = self.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(GithubError::Status { status: resp.status().as_u16(), url });
        }
        Ok(resp.json::<User>().await?.login)
    }

    /// All repositories the user can see. `include_orgs` controls whether
    /// organization repositories are included alongside their own.
    pub async fn list_repos(&self, include_orgs: bool) -> Result<Vec<Repo>, GithubError> {
        let affiliation = if include_orgs {
            "owner,organization_member"
        } else {
            "owner"
        };
        let mut all = Vec::new();
        let mut page = 1;
        loop {
            let url = format!("{}/user/repos", self.base_url);
            let resp = self
                .get(&url)
                .query(&[
                    ("per_page", REPOS_PER_PAGE.to_string()),
                    ("page", page.to_string()),
                    ("affiliation", affiliation.to_string()),
                    ("sort", "pushed".to_string()),
                ])
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(GithubError::Status { status: resp.status().as_u16(), url });
            }
            let batch: Vec<Repo> = resp.json().await?;
            let done = batch.len() < REPOS_PER_PAGE;
            all.extend(batch);
            if done {
                return Ok(all);
            }
            page += 1;
        }
    }

    pub async fn list_runs(
        &self,
        full_name: &str,
        etag: Option<&str>,
    ) -> Result<RunsResponse, GithubError> {
        let url = format!("{}/repos/{}/actions/runs", self.base_url, full_name);
        let mut req = self
            .get(&url)
            .query(&[("per_page", RUNS_PER_REPO.to_string())]);
        if let Some(tag) = etag {
            req = req.header("if-none-match", tag);
        }
        let resp = req.send().await?;

        let rate_limit_remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());
        let new_etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if resp.status().as_u16() == 304 {
            return Ok(RunsResponse {
                runs: Vec::new(),
                etag: new_etag,
                not_modified: true,
                rate_limit_remaining,
            });
        }
        if !resp.status().is_success() {
            return Err(GithubError::Status { status: resp.status().as_u16(), url });
        }
        let body: RunsBody = resp.json().await?;
        Ok(RunsResponse {
            runs: body.workflow_runs,
            etag: new_etag,
            not_modified: false,
            rate_limit_remaining,
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test github::` Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/github.rs
git commit -m "feat: GitHub API client with ETag and rate-limit reporting"
```

---

### Task 7: Sync cycle

**Files:**

- Modify: `src/sync.rs`

`SyncState` is the shared status the header displays. `sync_once` runs one full cycle;
per-repository errors are counted and logged, never fatal.

- [ ] **Step 1: Write the failing tests**

Append to `src/sync.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn runs_body(id: i64, branch: &str, login: &str, actor_type: &str) -> serde_json::Value {
        serde_json::json!({
            "workflow_runs": [{
                "id": id,
                "name": "test.yml",
                "head_branch": branch,
                "status": "completed",
                "conclusion": "success",
                "head_sha": "abc123",
                "html_url": format!("https://github.com/autarch/a/actions/runs/{id}"),
                "run_started_at": "2026-08-04T10:00:00Z",
                "updated_at": "2026-08-04T10:05:00Z",
                "actor": {"login": login, "type": actor_type},
                "head_commit": {"message": "Do a thing"}
            }]
        })
    }

    #[tokio::test]
    async fn stores_kept_runs_and_drops_bot_runs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(runs_body(1, "main", "autarch", "User")))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/b/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(runs_body(2, "dependabot/x", "dependabot[bot]", "Bot")),
            )
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/a", "main").unwrap();
        store.upsert_repo("autarch/b", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;

        let rows = store.recent_runs(&crate::store::RunQuery::default()).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(state.snapshot().error_count, 0);
    }

    #[tokio::test]
    async fn one_failing_repo_does_not_stop_the_cycle() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/gone/actions/runs"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/ok/actions/runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(runs_body(7, "main", "autarch", "User")))
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/gone", "main").unwrap();
        store.upsert_repo("autarch/ok", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;

        let rows = store.recent_runs(&crate::store::RunQuery::default()).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![7]);
        assert_eq!(state.snapshot().error_count, 1);
        assert!(state.snapshot().last_success.is_some());
    }

    #[tokio::test]
    async fn stores_etag_and_sends_it_next_time() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"abc\"")
                    .set_body_json(runs_body(1, "main", "autarch", "User")),
            )
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/a", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;
        assert_eq!(store.repo_etag("autarch/a").unwrap(), Some("W/\"abc\"".to_string()));
    }

    #[tokio::test]
    async fn discovery_marks_ignored_repos() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"full_name": "autarch/precious", "default_branch": "main"},
                {"full_name": "autarch/old-junk", "default_branch": "main"}
            ])))
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let cfg = crate::config::Config::from_toml(r#"ignore = ["autarch/old-*"]"#).unwrap();

        discover_repos(&client, &store, &cfg).await.unwrap();

        let names: Vec<_> =
            store.active_repos().unwrap().into_iter().map(|r| r.full_name).collect();
        assert_eq!(names, vec!["autarch/precious".to_string()]);
    }

    #[test]
    fn poll_interval_doubles_when_rate_limit_is_low() {
        assert_eq!(effective_interval(180, Some(4000)), 180);
        assert_eq!(effective_interval(180, Some(499)), 360);
        assert_eq!(effective_interval(180, None), 180);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sync::` Expected: FAIL — `SyncState` not found.

- [ ] **Step 3: Write the implementation**

Put this above the `#[cfg(test)]` block in `src/sync.rs`:

```rust
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::Serialize;

use crate::config::Config;
use crate::filter::{should_keep, RunCandidate};
use crate::github::Client;
use crate::store::{Store, StoredRun};

/// Below this many remaining API calls, back off.
const RATE_LIMIT_FLOOR: i64 = 500;
/// How long runs are kept.
const RETENTION_DAYS: i64 = 30;
/// How often repository discovery runs, relative to the run sync.
pub const DISCOVERY_INTERVAL_SECS: u64 = 3600;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStatus {
    pub last_success: Option<String>,
    pub error_count: usize,
    pub rate_limit_remaining: Option<i64>,
    pub last_error: Option<String>,
}

/// Shared, cheaply cloneable sync status for the header.
#[derive(Clone, Default)]
pub struct SyncState {
    inner: Arc<Mutex<SyncStatus>>,
}

impl SyncState {
    pub fn snapshot(&self) -> SyncStatus {
        self.inner.lock().unwrap().clone()
    }

    fn record_success(&self, rate_limit: Option<i64>) {
        let mut s = self.inner.lock().unwrap();
        s.last_success = Some(Utc::now().to_rfc3339());
        if rate_limit.is_some() {
            s.rate_limit_remaining = rate_limit;
        }
    }

    fn record_error(&self, msg: String) {
        let mut s = self.inner.lock().unwrap();
        s.error_count += 1;
        s.last_error = Some(msg);
    }

    fn reset_errors(&self) {
        self.inner.lock().unwrap().error_count = 0;
    }
}

/// Double the interval when the rate limit is running low.
pub fn effective_interval(base_secs: u64, remaining: Option<i64>) -> u64 {
    match remaining {
        Some(r) if r < RATE_LIMIT_FLOOR => base_secs * 2,
        _ => base_secs,
    }
}

/// Fetch the repository list and record it, marking ignored ones.
pub async fn discover_repos(client: &Client, store: &Store, cfg: &Config) -> Result<()> {
    let matcher = cfg.ignore_matcher()?;
    let repos = client.list_repos(cfg.include_orgs).await?;
    for r in repos {
        store.upsert_repo(&r.full_name, &r.default_branch)?;
        store.set_repo_ignored(&r.full_name, matcher.is_ignored(&r.full_name))?;
    }
    Ok(())
}

/// One pass over every active repository. Errors are counted, never fatal.
pub async fn sync_runs(client: &Client, store: &Store, state: &SyncState, current_user: &str) {
    let repos = match store.active_repos() {
        Ok(r) => r,
        Err(e) => {
            state.record_error(format!("cannot read repositories: {e}"));
            return;
        }
    };

    state.reset_errors();
    let mut last_rate_limit = None;

    for repo in repos {
        let etag = store.repo_etag(&repo.full_name).ok().flatten();
        let resp = match client.list_runs(&repo.full_name, etag.as_deref()).await {
            Ok(resp) => resp,
            Err(e) => {
                let msg = if e.is_unauthorized() {
                    "GitHub rejected the token — run `gh auth login` to refresh it".to_string()
                } else {
                    format!("{}: {e}", repo.full_name)
                };
                tracing::warn!("{msg}");
                state.record_error(msg);
                continue;
            }
        };

        if resp.rate_limit_remaining.is_some() {
            last_rate_limit = resp.rate_limit_remaining;
        }
        if let Some(tag) = &resp.etag {
            let _ = store.set_repo_etag(&repo.full_name, tag);
        }

        for run in resp.runs {
            let candidate = RunCandidate {
                branch: run.head_branch.clone(),
                actor_login: run.actor.login.clone(),
                actor_type: run.actor.r#type.clone(),
            };
            if !should_keep(&candidate, &repo.default_branch, current_user) {
                continue;
            }
            let stored = StoredRun {
                id: run.id,
                repo_full_name: repo.full_name.clone(),
                workflow_name: run.workflow_name.clone(),
                branch: run.head_branch.clone(),
                actor: run.actor.login.clone(),
                status: run.status.clone(),
                conclusion: run.conclusion.clone(),
                commit_sha: run.head_sha.clone(),
                commit_subject: run.commit_subject(),
                html_url: run.html_url.clone(),
                started_at: run.started_at(),
            };
            if let Err(e) = store.upsert_run(&stored) {
                tracing::warn!("failed to store run {}: {e}", run.id);
                state.record_error(format!("failed to store run {}: {e}", run.id));
            }
        }
    }

    let cutoff = (Utc::now() - Duration::days(RETENTION_DAYS)).to_rfc3339();
    if let Err(e) = store.prune_before(&cutoff) {
        tracing::warn!("prune failed: {e}");
    }

    state.record_success(last_rate_limit);
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test sync::` Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/sync.rs
git commit -m "feat: sync cycle with discovery, filtering, and pruning"
```

---

### Task 8: HTTP server and page assets

**Files:**

- Modify: `src/server.rs`
- Create: `src/assets/index.html`
- Create: `src/assets/app.css`
- Create: `src/assets/app.js`
- Create: `tests/api.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/api.rs`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use gh_web_dash::server::{router, AppState};
use gh_web_dash::store::{Store, StoredRun};
use gh_web_dash::sync::SyncState;

fn seeded_state() -> AppState {
    let store = Store::open_in_memory().unwrap();
    store.upsert_repo("autarch/precious", "main").unwrap();
    for (id, workflow, conclusion, started) in [
        (1_i64, "test.yml", Some("success"), "2026-08-04T09:00:00Z"),
        (2, "test.yml", Some("failure"), "2026-08-04T10:00:00Z"),
        (3, "release.yml", Some("success"), "2026-08-04T11:00:00Z"),
    ] {
        store
            .upsert_run(&StoredRun {
                id,
                repo_full_name: "autarch/precious".into(),
                workflow_name: workflow.into(),
                branch: "main".into(),
                actor: "autarch".into(),
                status: "completed".into(),
                conclusion: conclusion.map(|s| s.to_string()),
                commit_sha: "abc123".into(),
                commit_subject: "Do a thing".into(),
                html_url: format!("https://github.com/autarch/precious/actions/runs/{id}"),
                started_at: started.into(),
            })
            .unwrap();
    }
    AppState { store, sync: SyncState::default(), trigger: tokio::sync::mpsc::channel(1).0 }
}

async fn get_json(path: &str) -> serde_json::Value {
    let resp = router(seeded_state())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn index_serves_html() {
    let resp = router(seeded_state())
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("<table"), "index should render a table");
}

#[tokio::test]
async fn runs_are_newest_first() {
    let v = get_json("/api/runs").await;
    let ids: Vec<i64> = v["runs"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![3, 2, 1]);
}

#[tokio::test]
async fn workflow_names_are_included() {
    let v = get_json("/api/runs").await;
    let names: Vec<&str> =
        v["workflows"].as_array().unwrap().iter().map(|w| w.as_str().unwrap()).collect();
    assert_eq!(names, vec!["release.yml", "test.yml"]);
}

#[tokio::test]
async fn failures_only_filters() {
    let v = get_json("/api/runs?failures_only=true").await;
    let ids: Vec<i64> = v["runs"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![2]);
}

#[tokio::test]
async fn workflow_filter_applies() {
    let v = get_json("/api/runs?workflow=release.yml").await;
    let ids: Vec<i64> = v["runs"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![3]);
}

#[tokio::test]
async fn status_reports_sync_state() {
    let v = get_json("/api/status").await;
    assert!(v.get("error_count").is_some());
    assert!(v.get("last_success").is_some());
}

#[tokio::test]
async fn sync_endpoint_accepts_a_trigger() {
    let resp = router(seeded_state())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test api` Expected: FAIL — no library target / `gh_web_dash` not found.

- [ ] **Step 3: Add a library target so integration tests can import the modules**

Create `src/lib.rs`:

```rust
pub mod auth;
pub mod config;
pub mod filter;
pub mod github;
pub mod server;
pub mod store;
pub mod sync;
```

Replace the module declarations at the top of `src/main.rs` with:

```rust
use gh_web_dash::{auth, config, github, server, store, sync};
```

(The rest of `main.rs` is written in Task 9; for now keep its `fn main()` body as the placeholder
`println!`, and add `#[allow(unused_imports)]` above the `use` if the build warns.)

- [ ] **Step 4: Write the server implementation**

Put this in `src/server.rs`:

```rust
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::store::{RunQuery, Store, StoredRun};
use crate::sync::{SyncState, SyncStatus};

const MAX_ROWS: usize = 200;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_JS: &str = include_str!("assets/app.js");

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub sync: SyncState,
    /// Sending on this asks the poll loop to run a cycle immediately.
    pub trigger: Sender<()>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RunsParams {
    #[serde(default)]
    pub failures_only: Option<bool>,
    pub workflow: Option<String>,
    pub repo: Option<String>,
}

#[derive(Serialize)]
pub struct RunsResponse {
    pub runs: Vec<StoredRun>,
    pub workflows: Vec<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(app_css))
        .route("/app.js", get(app_js))
        .route("/api/runs", get(runs))
        .route("/api/status", get(status))
        .route("/api/sync", post(trigger_sync))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], APP_CSS)
}

async fn app_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript")], APP_JS)
}

async fn runs(
    State(state): State<AppState>,
    Query(params): Query<RunsParams>,
) -> Result<Json<RunsResponse>, StatusCode> {
    let q = RunQuery {
        failures_only: params.failures_only.unwrap_or(false),
        workflow: params.workflow,
        repo: params.repo,
        limit: MAX_ROWS,
    };
    let runs = state.store.recent_runs(&q).map_err(|e| {
        tracing::error!("query failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let workflows = state.store.workflow_names().unwrap_or_default();
    Ok(Json(RunsResponse { runs, workflows }))
}

async fn status(State(state): State<AppState>) -> Json<SyncStatus> {
    Json(state.sync.snapshot())
}

async fn trigger_sync(State(state): State<AppState>) -> StatusCode {
    // A full channel means a sync is already queued — that is success, not an error.
    let _ = state.trigger.try_send(());
    StatusCode::ACCEPTED
}
```

- [ ] **Step 5: Write the page assets**

Create `src/assets/index.html`:

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>gh-web-dash</title>
    <link rel="stylesheet" href="/app.css" />
  </head>
  <body>
    <header>
      <h1>gh-web-dash</h1>
      <div id="chips" class="chips"></div>
      <div class="spacer"></div>
      <span id="staleness" class="status"></span>
      <span id="ratelimit" class="status warn hidden"></span>
      <button id="refresh" type="button">Refresh now</button>
    </header>
    <table>
      <tbody id="rows"></tbody>
    </table>
    <p id="empty" class="empty hidden">No runs yet — the first sync is still working.</p>
    <script src="/app.js"></script>
  </body>
</html>
```

Create `src/assets/app.css`:

```css
:root {
  --bg: #fff;
  --fg: #1f2328;
  --muted: #59636e;
  --border: #d1d9e0;
  --ok: #1a7f37;
  --bad: #cf222e;
  --run: #bf8700;
  --warn: #9a6700;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117;
    --fg: #e6edf3;
    --muted: #9198a1;
    --border: #30363d;
  }
}
* {
  box-sizing: border-box;
}
body {
  margin: 0;
  background: var(--bg);
  color: var(--fg);
  font:
    13px/1.4 -apple-system,
    "Segoe UI",
    system-ui,
    sans-serif;
}
header {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 0;
  background: var(--bg);
  flex-wrap: wrap;
}
h1 {
  font-size: 14px;
  margin: 0 8px 0 0;
}
.spacer {
  flex: 1;
}
.chips {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.chip {
  border: 1px solid var(--border);
  border-radius: 99px;
  padding: 2px 10px;
  font-size: 11px;
  cursor: pointer;
  background: none;
  color: inherit;
}
.chip.on {
  background: #0969da;
  border-color: #0969da;
  color: #fff;
}
.status {
  color: var(--muted);
  font-size: 12px;
}
.status.stale,
.status.warn {
  color: var(--warn);
  font-weight: 600;
}
.hidden {
  display: none;
}
button#refresh {
  border: 1px solid var(--border);
  border-radius: 6px;
  background: none;
  color: inherit;
  padding: 3px 10px;
  cursor: pointer;
}
table {
  width: 100%;
  border-collapse: collapse;
}
tr {
  border-bottom: 1px solid var(--border);
}
tr:hover {
  background: rgba(127, 127, 127, 0.08);
}
td {
  padding: 6px 10px;
  white-space: nowrap;
}
td a {
  color: inherit;
  text-decoration: none;
  display: block;
}
.dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  display: inline-block;
}
.dot.ok {
  background: var(--ok);
}
.dot.bad {
  background: var(--bad);
}
.dot.run {
  background: var(--run);
}
.dot.other {
  background: var(--muted);
}
.repo {
  font-weight: 600;
}
.muted {
  color: var(--muted);
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
}
td.time {
  text-align: right;
  color: var(--muted);
  width: 1%;
}
.empty {
  padding: 20px;
  color: var(--muted);
}
```

Create `src/assets/app.js`:

```js
"use strict";

const POLL_MS = 15000;
const STALE_AFTER_MS = 3 * 180 * 1000; // three default poll intervals

let failuresOnly = false;
let workflow = null;
let lastGood = null;

function dotClass(run) {
  if (run.status !== "completed") return "run";
  if (run.conclusion === "success") return "ok";
  if (["failure", "timed_out", "startup_failure"].includes(run.conclusion)) return "bad";
  return "other";
}

function relTime(iso) {
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return Math.floor(secs) + "s ago";
  if (secs < 3600) return Math.floor(secs / 60) + "m ago";
  if (secs < 86400) return Math.floor(secs / 3600) + "h ago";
  return Math.floor(secs / 86400) + "d ago";
}

function renderChips(workflows) {
  const chips = document.getElementById("chips");
  const wanted = ["__all__", "__failures__"].concat(workflows);
  // Rebuild only when the set of chips changed, so clicks are not lost mid-poll.
  if (chips.dataset.keys === wanted.join("�")) {
    updateChipState();
    return;
  }
  chips.dataset.keys = wanted.join("�");
  chips.innerHTML = "";
  chips.appendChild(
    makeChip(
      "All",
      () => {
        failuresOnly = false;
        workflow = null;
      },
      "all",
    ),
  );
  chips.appendChild(
    makeChip(
      "Failures only",
      () => {
        failuresOnly = true;
      },
      "failures",
    ),
  );
  for (const w of workflows) {
    chips.appendChild(
      makeChip(
        w,
        () => {
          workflow = workflow === w ? null : w;
        },
        "wf:" + w,
      ),
    );
  }
  updateChipState();
}

function makeChip(label, onClick, key) {
  const b = document.createElement("button");
  b.className = "chip";
  b.type = "button";
  b.textContent = label;
  b.dataset.key = key;
  b.addEventListener("click", () => {
    onClick();
    updateChipState();
    load();
  });
  return b;
}

function updateChipState() {
  for (const b of document.querySelectorAll(".chip")) {
    const k = b.dataset.key;
    let on = false;
    if (k === "all") on = !failuresOnly && workflow === null;
    else if (k === "failures") on = failuresOnly;
    else if (k.startsWith("wf:")) on = workflow === k.slice(3);
    b.classList.toggle("on", on);
  }
}

function renderRows(runs) {
  const tbody = document.getElementById("rows");
  tbody.innerHTML = "";
  for (const run of runs) {
    const tr = document.createElement("tr");
    const cells = [
      ['<span class="dot ' + dotClass(run) + '"></span>', ""],
      [run.repo_full_name, "repo"],
      [run.workflow_name, "mono muted"],
      [run.branch, "mono muted"],
      [relTime(run.started_at), ""],
    ];
    cells.forEach(([content, cls], i) => {
      const td = document.createElement("td");
      if (i === 4) td.className = "time";
      const a = document.createElement("a");
      a.href = run.html_url;
      a.target = "_blank";
      a.rel = "noopener";
      if (i === 0) a.innerHTML = content;
      else {
        a.textContent = content;
        a.className = cls;
      }
      td.appendChild(a);
      tr.appendChild(td);
    });
    tbody.appendChild(tr);
  }
  document.getElementById("empty").classList.toggle("hidden", runs.length > 0);
}

async function load() {
  const params = new URLSearchParams();
  if (failuresOnly) params.set("failures_only", "true");
  if (workflow) params.set("workflow", workflow);
  try {
    const resp = await fetch("/api/runs?" + params.toString());
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const data = await resp.json();
    lastGood = Date.now();
    renderChips(data.workflows);
    renderRows(data.runs);
  } catch (e) {
    // Leave the last-good table on screen; the header will show staleness.
    console.warn("runs fetch failed", e);
  }
  await loadStatus();
}

async function loadStatus() {
  const el = document.getElementById("staleness");
  const rl = document.getElementById("ratelimit");
  try {
    const resp = await fetch("/api/status");
    if (!resp.ok) throw new Error("HTTP " + resp.status);
    const s = await resp.json();
    if (!s.last_success) {
      el.textContent = "syncing…";
      el.classList.remove("stale");
    } else {
      const age = Date.now() - new Date(s.last_success).getTime();
      el.textContent = "synced " + relTime(s.last_success);
      el.classList.toggle("stale", age > STALE_AFTER_MS);
    }
    const low = s.rate_limit_remaining !== null && s.rate_limit_remaining < 500;
    rl.classList.toggle("hidden", !low);
    if (low) rl.textContent = "rate limit low (" + s.rate_limit_remaining + ")";
  } catch (e) {
    el.textContent = "server unreachable";
    el.classList.add("stale");
  }
}

document.getElementById("refresh").addEventListener("click", async () => {
  await fetch("/api/sync", { method: "POST" });
  setTimeout(load, 1500);
});

load();
setInterval(load, POLL_MS);
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --test api` Expected: 7 tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/lib.rs src/main.rs src/server.rs src/assets tests/api.rs
git commit -m "feat: HTTP server and dashboard page"
```

---

### Task 9: Wire it together

**Files:**

- Modify: `src/main.rs`

Binds an ephemeral port, opens the browser, and runs the poll loop until killed.

- [ ] **Step 1: Write `src/main.rs`**

```rust
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::mpsc;

use gh_web_dash::config::{default_config_path, default_db_path, Config};
use gh_web_dash::github::{Client, GITHUB_API};
use gh_web_dash::server::{router, AppState};
use gh_web_dash::store::Store;
use gh_web_dash::sync::{discover_repos, effective_interval, sync_runs, SyncState,
                        DISCOVERY_INTERVAL_SECS};

#[derive(Parser)]
#[command(about = "A local dashboard of recent GitHub Actions runs")]
struct Args {
    /// Do not open a browser on startup.
    #[arg(long)]
    no_open: bool,
    /// Path to the config file.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gh_web_dash=info".into()),
        )
        .init();

    let args = Args::parse();

    // Startup failures are fatal and must name the fix.
    let config_path = match args.config {
        Some(p) => p,
        None => default_config_path()?,
    };
    let cfg = Config::load_or_create(&config_path)
        .with_context(|| format!("failed to load config from {}", config_path.display()))?;
    let token = gh_web_dash::auth::resolve_token().await?;
    let store = Store::open(&default_db_path()?)?;
    let client = Client::new(GITHUB_API.to_string(), token)?;

    let current_user = client
        .current_user()
        .await
        .context("could not identify you to GitHub — is the token valid?")?;
    tracing::info!("authenticated as {current_user}");

    let sync_state = SyncState::default();
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(1);

    // Background poll loop. Discovery runs on its own slower cadence.
    {
        let client = client.clone();
        let store = store.clone();
        let sync_state = sync_state.clone();
        let cfg = cfg.clone();
        let user = current_user.clone();
        tokio::spawn(async move {
            let mut last_discovery: Option<std::time::Instant> = None;
            loop {
                let due = last_discovery
                    .map(|t| t.elapsed().as_secs() >= DISCOVERY_INTERVAL_SECS)
                    .unwrap_or(true);
                if due {
                    match discover_repos(&client, &store, &cfg).await {
                        Ok(()) => last_discovery = Some(std::time::Instant::now()),
                        Err(e) => tracing::warn!("repository discovery failed: {e}"),
                    }
                }

                sync_runs(&client, &store, &sync_state, &user).await;

                let secs = effective_interval(
                    cfg.poll_interval_secs,
                    sync_state.snapshot().rate_limit_remaining,
                );
                // Wake early if the browser asked for a sync.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
                    _ = trigger_rx.recv() => {}
                }
            }
        });
    }

    let state = AppState { store, sync: sync_state, trigger: trigger_tx };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("could not bind a local port")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    println!("gh-web-dash listening on {url}");

    if !args.no_open {
        if let Err(e) = open::that_detached(&url) {
            tracing::warn!("could not open a browser ({e}) — visit {url}");
        }
    }

    axum::serve(listener, router(state)).await.context("server error")?;
    Ok(())
}
```

- [ ] **Step 2: Verify the whole suite builds and passes**

Run: `cargo test` Expected: all tests across `config`, `auth`, `filter`, `store`, `github`, `sync`,
and `tests/api.rs` pass.

- [ ] **Step 3: Check formatting and lints**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings` Expected: clean. Fix anything
reported.

- [ ] **Step 4: Manual smoke test**

Run: `cargo run`

Expected: a browser opens on `http://127.0.0.1:<random port>`; the header shows "syncing…" and then
"synced Ns ago"; rows appear within a couple of minutes as the first sync completes. Click a row —
it opens the run on github.com in a new tab. Click "Failures only" — the table filters. Click
"Refresh now" — the synced-at time resets.

If the first sync is slow, that is expected: ~100 repositories with cold ETags. Subsequent cycles
are mostly 304s.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire up server, poll loop, and browser launch"
```

---

### Task 10: README

**Files:**

- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

```markdown
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

## Configuration

`~/.config/gh-web-dash/config.toml`, created with defaults on first run:

    poll_interval_secs = 180
    include_orgs = true
    ignore = ["you/old-*"]

`ignore` holds globs matched against `owner/repo`.

## What it shows

A time-ordered feed of workflow runs on each repository's default branch, plus runs on branches you
authored. Bot-authored runs are excluded. Clicking a row opens the run on github.com.

Data is cached in `~/.config/gh-web-dash/runs.db` and pruned after 30 days.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add README"
```

---

## Notes for the implementer

- **Version drift.** The dependency versions above were current when this plan was written. If
  `cargo build` reports an API mismatch — particularly for `axum`, `rusqlite`, or `thiserror` —
  check the crate's docs for the version cargo resolved rather than pinning backwards.
- **`Store` is `Clone`.** It clones the `Arc<Mutex<Connection>>`, so every clone shares one
  connection. That is intentional; do not swap in a pool without a reason.
- **Do not add features not in the plan.** Inline log drill-down, run re-triggering, and historical
  analytics are explicitly out of scope for v1.
