use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::Serialize;

use crate::config::Config;
use crate::filter::{should_keep, RunCandidate};
use crate::github::Client;
use crate::inclusion::{decide, Decision, Override, RepoFacts, SkipReason};
use crate::store::{Store, StoredRun};

/// Below this many remaining API calls, back off.
const RATE_LIMIT_FLOOR: i64 = 500;
/// How long runs are kept.
const RETENTION_DAYS: i64 = 30;
/// How often repository discovery runs, relative to the run sync.
pub const DISCOVERY_INTERVAL_SECS: u64 = 3600;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncStatus {
    /// Set while a cycle is running, `None` when idle. The page shows this
    /// instead of guessing from when the refresh button was pressed, so the
    /// indicator also covers scheduled cycles and cannot get stuck.
    pub progress: Option<SyncProgress>,
    pub last_success: Option<String>,
    pub error_count: usize,
    pub rate_limit_remaining: Option<i64>,
    pub last_error: Option<String>,
    /// The poll interval actually in effect for the next cycle, in seconds
    /// (after any low-rate-limit backoff). The UI derives its staleness
    /// threshold from this so backoff itself doesn't trip the alarm.
    pub poll_interval_secs: Option<u64>,
}

/// How far through a cycle the poller is, counted in repositories: a 304 is
/// as much progress as a full fetch, so the count moves steadily.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SyncProgress {
    pub done: usize,
    pub total: usize,
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

    fn begin_cycle(&self, total: usize) {
        self.inner.lock().unwrap().progress = Some(SyncProgress { done: 0, total });
    }

    fn advance_cycle(&self) {
        if let Some(p) = self.inner.lock().unwrap().progress.as_mut() {
            p.done += 1;
        }
    }

    fn end_cycle(&self) {
        self.inner.lock().unwrap().progress = None;
    }

    /// Record the poll interval that will govern the next cycle's wait, so
    /// the UI can size its staleness threshold off the real value instead of
    /// a hardcoded default.
    pub fn record_poll_interval(&self, secs: u64) {
        self.inner.lock().unwrap().poll_interval_secs = Some(secs);
    }
}

/// Double the interval when the rate limit is running low.
pub fn effective_interval(base_secs: u64, remaining: Option<i64>) -> u64 {
    match remaining {
        Some(r) if r < RATE_LIMIT_FLOOR => base_secs * 2,
        _ => base_secs,
    }
}

/// Re-evaluate one repository's inclusion and store the result. Used by
/// discovery, and by the UI so a toggle takes effect now rather than within
/// the hour.
pub fn apply_decision(store: &Store, cfg: &Config, full_name: &str) -> Result<Decision> {
    let matcher = cfg.ignore_matcher()?;
    let facts = store
        .repo_facts(full_name)?
        .ok_or_else(|| anyhow::anyhow!("unknown repository: {full_name}"))?;
    let decision = decide(
        &RepoFacts {
            user_override: facts.user_override.as_deref().and_then(Override::parse),
            glob_ignored: matcher.is_ignored(full_name),
            archived: facts.archived,
            pushed_at: facts.pushed_at.as_deref(),
            has_runs: facts.has_runs,
        },
        Utc::now(),
    );
    store.set_repo_decision(
        full_name,
        decision.is_included(),
        decision.skip_reason().map(|r| r.as_str()),
    )?;
    Ok(decision)
}

/// Fetch the repository list and record it, marking ignored ones.
pub async fn discover_repos(client: &Client, store: &Store, cfg: &Config) -> Result<()> {
    let repos = client.list_repos(cfg.include_orgs).await?;
    let seen: std::collections::HashSet<String> =
        repos.iter().map(|r| r.full_name.clone()).collect();
    for r in &repos {
        store.upsert_repo(&r.full_name, &r.default_branch)?;
        store.set_repo_facts(&r.full_name, r.archived, r.pushed_at.as_deref())?;
        apply_decision(store, cfg, &r.full_name)?;
    }
    // A repository that is no longer returned by discovery has been deleted,
    // renamed, or the token has lost access — either way, stop polling it.
    for name in store.all_repo_names()? {
        if !seen.contains(&name) {
            store.set_repo_decision(&name, false, Some(SkipReason::Gone.as_str()))?;
        }
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
    state.begin_cycle(repos.len());
    let mut last_rate_limit = None;
    let mut any_success = false;

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
                state.advance_cycle();
                continue;
            }
        };

        any_success = true;
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
                workflow_path: run.path.clone(),
            };
            if let Err(e) = store.upsert_run(&stored) {
                tracing::warn!("failed to store run {}: {e}", run.id);
                state.record_error(format!("failed to store run {}: {e}", run.id));
            }
        }

        state.advance_cycle();
    }

    let cutoff = (Utc::now() - Duration::days(RETENTION_DAYS)).to_rfc3339();
    if let Err(e) = store.prune_before(&cutoff) {
        tracing::warn!("prune failed: {e}");
    }

    if any_success {
        state.record_success(last_rate_limit);
    }
    state.end_cycle();
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Every run id stored across all active repositories, in whatever order
    /// the history query returns them — good enough for asserting which runs
    /// made it into the store.
    fn all_run_ids(store: &Store) -> Vec<i64> {
        let mut ids: Vec<i64> = store
            .active_repos()
            .unwrap()
            .into_iter()
            .flat_map(|r| store.repo_history(&r.full_name, 100).unwrap())
            .flat_map(|h| h.runs)
            .map(|r| r.id)
            .collect();
        ids.sort_unstable();
        ids
    }

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
    async fn progress_is_cleared_when_a_cycle_ends() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(runs_body(1, "main", "autarch", "User")),
            )
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/a", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        assert!(state.snapshot().progress.is_none(), "idle before the cycle");
        sync_runs(&client, &store, &state, "autarch").await;
        assert!(
            state.snapshot().progress.is_none(),
            "idle again after the cycle"
        );
    }

    #[tokio::test]
    async fn progress_counts_every_repo_including_failures() {
        let server = MockServer::start().await;
        // Delayed so the cycle lasts long enough to observe mid-flight;
        // without this the whole sync finishes inside one sampling tick.
        Mock::given(method("GET"))
            .and(path("/repos/autarch/ok/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(runs_body(1, "main", "autarch", "User"))
                    .set_delay(std::time::Duration::from_millis(60)),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/gone/actions/runs"))
            .respond_with(
                ResponseTemplate::new(404).set_delay(std::time::Duration::from_millis(60)),
            )
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/ok", "main").unwrap();
        store.upsert_repo("autarch/gone", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        // Observe progress from another task while the cycle runs.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let state = state.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                for _ in 0..200 {
                    if let Some(p) = state.snapshot().progress {
                        seen.lock().unwrap().push((p.done, p.total));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            });
        }

        sync_runs(&client, &store, &state, "autarch").await;

        let seen = seen.lock().unwrap().clone();
        assert!(!seen.is_empty(), "progress should be visible mid-cycle");
        assert!(
            seen.iter().all(|(_, total)| *total == 2),
            "total is the repo count: {seen:?}"
        );
        // The failing repo still advances the counter — otherwise the number
        // would stall and look wedged.
        assert!(
            seen.iter().any(|(done, _)| *done > 0),
            "counter advances: {seen:?}"
        );
    }

    #[tokio::test]
    async fn stores_kept_runs_and_drops_bot_runs() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/a/actions/runs"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(runs_body(1, "main", "autarch", "User")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/b/actions/runs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(runs_body(
                2,
                "dependabot/x",
                "dependabot[bot]",
                "Bot",
            )))
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/a", "main").unwrap();
        store.upsert_repo("autarch/b", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;

        let ids = all_run_ids(&store);
        assert_eq!(ids, vec![1]);
        assert_eq!(state.snapshot().error_count, 0);
    }

    #[tokio::test]
    async fn all_repos_failing_leaves_last_success_unset() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/autarch/gone/actions/runs"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/gone", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;

        assert_eq!(state.snapshot().last_success, None);
        assert_eq!(state.snapshot().error_count, 1);
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
            .respond_with(
                ResponseTemplate::new(200).set_body_json(runs_body(7, "main", "autarch", "User")),
            )
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        store.upsert_repo("autarch/gone", "main").unwrap();
        store.upsert_repo("autarch/ok", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let state = SyncState::default();

        sync_runs(&client, &store, &state, "autarch").await;

        let ids = all_run_ids(&store);
        assert_eq!(ids, vec![7]);
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
        assert_eq!(
            store.repo_etag("autarch/a").unwrap(),
            Some("W/\"abc\"".to_string())
        );
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

        let names: Vec<_> = store
            .active_repos()
            .unwrap()
            .into_iter()
            .map(|r| r.full_name)
            .collect();
        assert_eq!(names, vec!["autarch/precious".to_string()]);
    }

    #[tokio::test]
    async fn discovery_ignores_repos_no_longer_returned() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"full_name": "autarch/precious", "default_branch": "main"}
            ])))
            .mount(&server)
            .await;

        let store = crate::store::Store::open_in_memory().unwrap();
        // Simulate a repo stored from an earlier cycle that has since vanished
        // (deleted, or access revoked).
        store.upsert_repo("autarch/vanished", "main").unwrap();
        let client = crate::github::Client::new(server.uri(), "t".into()).unwrap();
        let cfg = crate::config::Config::from_toml("").unwrap();

        discover_repos(&client, &store, &cfg).await.unwrap();

        let names: Vec<_> = store
            .active_repos()
            .unwrap()
            .into_iter()
            .map(|r| r.full_name)
            .collect();
        assert_eq!(names, vec!["autarch/precious".to_string()]);
    }

    #[test]
    fn poll_interval_is_recorded_for_the_status_endpoint() {
        let state = SyncState::default();
        assert_eq!(state.snapshot().poll_interval_secs, None);
        state.record_poll_interval(600);
        assert_eq!(state.snapshot().poll_interval_secs, Some(600));
    }

    #[test]
    fn poll_interval_doubles_when_rate_limit_is_low() {
        assert_eq!(effective_interval(180, Some(4000)), 180);
        assert_eq!(effective_interval(180, Some(499)), 360);
        assert_eq!(effective_interval(180, None), 180);
    }
}
