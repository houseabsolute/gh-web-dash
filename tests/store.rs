use gh_web_dash::store::{Health, RepoSummary, Store, StoredRun};

/// A run with every field settable that matters to these tests.
fn run_full(
    id: i64,
    repo: &str,
    workflow: &str,
    status: &str,
    conclusion: Option<&str>,
    started: &str,
) -> StoredRun {
    StoredRun {
        id,
        repo_full_name: repo.to_string(),
        workflow_name: workflow.to_string(),
        branch: "main".to_string(),
        actor: "autarch".to_string(),
        status: status.to_string(),
        conclusion: conclusion.map(std::string::ToString::to_string),
        commit_sha: "abc123".to_string(),
        commit_subject: "Do a thing".to_string(),
        html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
        started_at: started.to_string(),
        workflow_path: Some(".github/workflows/lint.yml".to_string()),
    }
}

#[test]
fn upsert_is_idempotent_and_updates_in_place() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();

    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    assert_eq!(
        s.repo_history("autarch/precious", 10).unwrap()[0]
            .runs
            .len(),
        1
    );

    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "W",
        "completed",
        Some("failure"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    let h = s.repo_history("autarch/precious", 10).unwrap();
    assert_eq!(h[0].runs.len(), 1);
    assert_eq!(h[0].runs[0].conclusion.as_deref(), Some("failure"));
}

#[test]
fn prune_removes_only_old_runs() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "W",
        "completed",
        Some("success"),
        "2020-01-01T00:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "autarch/precious",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();

    let removed = s.prune_before("2026-07-05T00:00:00Z").unwrap();
    assert_eq!(removed, 1);
    let rows = &s.repo_history("autarch/precious", 10).unwrap()[0].runs;
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
}

#[test]
fn etag_round_trips_and_starts_empty() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    assert_eq!(s.repo_etag("autarch/precious").unwrap(), None);
    s.set_repo_etag("autarch/precious", "W/\"abc\"").unwrap();
    assert_eq!(
        s.repo_etag("autarch/precious").unwrap(),
        Some("W/\"abc\"".to_string())
    );
}

#[test]
fn etag_for_unknown_repo_is_none_not_an_error() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.repo_etag("autarch/never-seen").unwrap(), None);
}

#[test]
fn upsert_repo_updates_default_branch_without_losing_etag() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "master").unwrap();
    s.set_repo_etag("autarch/precious", "W/\"abc\"").unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    assert_eq!(
        s.repo_etag("autarch/precious").unwrap(),
        Some("W/\"abc\"".to_string())
    );
    assert_eq!(s.active_repos().unwrap()[0].default_branch, "main");
}

#[test]
fn ignored_repos_are_excluded_from_active_repos() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_repo("autarch/scratch", "main").unwrap();
    s.set_repo_ignored("autarch/scratch", true).unwrap();
    let names: Vec<_> = s
        .active_repos()
        .unwrap()
        .into_iter()
        .map(|r| r.full_name)
        .collect();
    assert_eq!(names, vec!["autarch/precious".to_string()]);
}

fn summaries(s: &Store, failures_only: bool) -> Vec<RepoSummary> {
    s.repo_summaries(failures_only).unwrap()
}

#[test]
fn health_orders_least_bad_to_worst() {
    assert!(Health::Success < Health::Neutral);
    assert!(Health::Neutral < Health::Running);
    assert!(Health::Running < Health::Failure);
}

#[test]
fn health_classifies_runs() {
    assert_eq!(Health::of("completed", Some("success")), Health::Success);
    assert_eq!(Health::of("completed", Some("failure")), Health::Failure);
    assert_eq!(Health::of("completed", Some("timed_out")), Health::Failure);
    assert_eq!(
        Health::of("completed", Some("startup_failure")),
        Health::Failure
    );
    assert_eq!(Health::of("completed", Some("cancelled")), Health::Neutral);
    assert_eq!(Health::of("completed", Some("skipped")), Health::Neutral);
    assert_eq!(Health::of("in_progress", None), Health::Running);
    assert_eq!(Health::of("queued", None), Health::Running);
}

#[test]
fn summary_takes_the_latest_run_of_each_workflow() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    // Older failure, newer success — the newer one wins.
    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "Run tests",
        "completed",
        Some("failure"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "autarch/precious",
        "Run tests",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        3,
        "autarch/precious",
        "Lint",
        "completed",
        Some("success"),
        "2026-08-04T08:00:00Z",
    ))
    .unwrap();

    let out = summaries(&s, false);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].workflows.len(), 2);
    let names: Vec<_> = out[0]
        .workflows
        .iter()
        .map(|w| w.workflow_name.clone())
        .collect();
    assert_eq!(names, vec!["Lint".to_string(), "Run tests".to_string()]);
    let tests = out[0]
        .workflows
        .iter()
        .find(|w| w.workflow_name == "Run tests")
        .unwrap();
    assert_eq!(tests.health, Health::Success);
    assert_eq!(out[0].health, Health::Success);
}

#[test]
fn repo_health_is_the_worst_workflow_even_when_it_is_the_older_one() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    // The failing workflow ran EARLIER than the passing one.
    s.upsert_run(&run_full(
        1,
        "autarch/precious",
        "Run tests",
        "completed",
        Some("failure"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "autarch/precious",
        "Lint",
        "completed",
        Some("success"),
        "2026-08-04T11:00:00Z",
    ))
    .unwrap();

    let out = summaries(&s, false);
    assert_eq!(out[0].health, Health::Failure);
    // Repo time is the NEWEST run across workflows, regardless of health.
    assert_eq!(out[0].started_at, "2026-08-04T11:00:00Z");
}

#[test]
fn running_outranks_success_but_not_failure() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/one",
        "W1",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "a/one",
        "W2",
        "in_progress",
        None,
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    assert_eq!(summaries(&s, false)[0].health, Health::Running);

    s.upsert_run(&run_full(
        3,
        "a/one",
        "W3",
        "completed",
        Some("failure"),
        "2026-08-04T08:00:00Z",
    ))
    .unwrap();
    assert_eq!(summaries(&s, false)[0].health, Health::Failure);
}

#[test]
fn repos_are_sorted_by_most_recent_activity() {
    let s = Store::open_in_memory().unwrap();
    for (i, (repo, started)) in [
        ("a/old", "2026-08-01T09:00:00Z"),
        ("a/newest", "2026-08-04T09:00:00Z"),
        ("a/middle", "2026-08-03T09:00:00Z"),
    ]
    .iter()
    .enumerate()
    {
        s.upsert_repo(repo, "main").unwrap();
        s.upsert_run(&run_full(
            i64::try_from(i).unwrap() + 1,
            repo,
            "W",
            "completed",
            Some("success"),
            started,
        ))
        .unwrap();
    }
    let names: Vec<_> = summaries(&s, false)
        .into_iter()
        .map(|r| r.full_name)
        .collect();
    assert_eq!(
        names,
        vec![
            "a/newest".to_string(),
            "a/middle".to_string(),
            "a/old".to_string()
        ]
    );
}

#[test]
fn repos_with_no_runs_do_not_appear() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/empty", "main").unwrap();
    assert!(summaries(&s, false).is_empty());
}

#[test]
fn ignored_repos_do_not_appear_in_summaries() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/kept", "main").unwrap();
    s.upsert_repo("a/hidden", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/kept",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "a/hidden",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    s.set_repo_ignored("a/hidden", true).unwrap();

    let names: Vec<_> = summaries(&s, false)
        .into_iter()
        .map(|r| r.full_name)
        .collect();
    assert_eq!(names, vec!["a/kept".to_string()]);
}

#[test]
fn failures_only_keeps_repos_whose_worst_workflow_failed() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/broken", "main").unwrap();
    s.upsert_repo("a/fine", "main").unwrap();
    s.upsert_repo("a/busy", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/broken",
        "W",
        "completed",
        Some("failure"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "a/fine",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        3,
        "a/busy",
        "W",
        "in_progress",
        None,
        "2026-08-04T11:00:00Z",
    ))
    .unwrap();

    let names: Vec<_> = summaries(&s, true)
        .into_iter()
        .map(|r| r.full_name)
        .collect();
    assert_eq!(names, vec!["a/broken".to_string()]);
}

#[test]
fn summary_carries_the_fields_the_row_displays() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    s.upsert_run(&run_full(
        7,
        "a/one",
        "Run tests",
        "completed",
        Some("failure"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();

    let w = &summaries(&s, false)[0].workflows[0];
    assert_eq!(w.branch, "main");
    assert_eq!(w.commit_subject, "Do a thing");
    assert_eq!(w.html_url, "https://github.com/a/one/actions/runs/7");
    assert_eq!(w.started_at, "2026-08-04T09:00:00Z");
}

#[test]
fn history_is_newest_first_per_workflow() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/one",
        "Run tests",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        2,
        "a/one",
        "Run tests",
        "completed",
        Some("failure"),
        "2026-08-04T11:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run_full(
        3,
        "a/one",
        "Lint",
        "completed",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();

    let h = s.repo_history("a/one", 10).unwrap();
    let names: Vec<_> = h.iter().map(|w| w.workflow_name.clone()).collect();
    assert_eq!(names, vec!["Lint".to_string(), "Run tests".to_string()]);

    let tests = h.iter().find(|w| w.workflow_name == "Run tests").unwrap();
    assert_eq!(
        tests.runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn history_limit_applies_per_workflow_not_per_repo() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    for i in 1..=5 {
        s.upsert_run(&run_full(
            i,
            "a/one",
            "W1",
            "completed",
            Some("success"),
            &format!("2026-08-04T0{i}:00:00Z"),
        ))
        .unwrap();
        s.upsert_run(&run_full(
            i + 100,
            "a/one",
            "W2",
            "completed",
            Some("success"),
            &format!("2026-08-04T0{i}:30:00Z"),
        ))
        .unwrap();
    }
    let h = s.repo_history("a/one", 2).unwrap();
    assert_eq!(h.len(), 2);
    for w in &h {
        assert_eq!(
            w.runs.len(),
            2,
            "workflow {} should be capped at 2",
            w.workflow_name
        );
    }
}

#[test]
fn history_of_an_ignored_or_unknown_repo_is_empty() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/hidden", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/hidden",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.set_repo_ignored("a/hidden", true).unwrap();

    assert!(s.repo_history("a/hidden", 10).unwrap().is_empty());
    assert!(s.repo_history("a/never-seen", 10).unwrap().is_empty());
}

#[test]
fn workflow_path_round_trips_and_reaches_summaries_and_history() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    s.upsert_run(&run_full(
        1,
        "a/one",
        "Run tests",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();

    assert_eq!(
        s.repo_summaries(false).unwrap()[0].workflows[0].workflow_path,
        Some(".github/workflows/lint.yml".to_string())
    );
    assert_eq!(
        s.repo_history("a/one", 10).unwrap()[0].runs[0].workflow_path,
        Some(".github/workflows/lint.yml".to_string())
    );
}

#[test]
fn runs_stored_before_the_migration_have_no_workflow_path() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("a/one", "main").unwrap();
    let mut r = run_full(
        1,
        "a/one",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    );
    r.workflow_path = None;
    s.upsert_run(&r).unwrap();

    assert_eq!(
        s.repo_summaries(false).unwrap()[0].workflows[0].workflow_path,
        None
    );
}

#[test]
fn migration_is_idempotent_and_clears_etags_only_once() {
    let dir = std::env::temp_dir().join(format!("ghwd-migration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("runs.db");

    // First open creates the schema fresh.
    {
        let s = Store::open(&path).unwrap();
        s.upsert_repo("a/one", "main").unwrap();
        s.set_repo_etag("a/one", "W/\"abc\"").unwrap();
    }
    // Re-opening must not error, must not re-run the migration, and so must
    // leave the ETag alone — otherwise every restart would force a full resync.
    {
        let s = Store::open(&path).unwrap();
        assert_eq!(s.repo_etag("a/one").unwrap(), Some("W/\"abc\"".to_string()));
        s.upsert_run(&run_full(
            1,
            "a/one",
            "W",
            "completed",
            Some("success"),
            "2026-08-04T09:00:00Z",
        ))
        .unwrap();
        assert_eq!(s.repo_summaries(false).unwrap().len(), 1);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn migration_adds_the_column_and_clears_etags_on_an_old_database() {
    let dir = std::env::temp_dir().join(format!("ghwd-oldmigration-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("runs.db");

    // Build a pre-migration database by hand: no workflow_id column.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (
                 full_name TEXT PRIMARY KEY, default_branch TEXT NOT NULL, etag TEXT,
                 last_synced TEXT, ignored INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE runs (
                 id INTEGER PRIMARY KEY,
                 repo_full_name TEXT NOT NULL REFERENCES repos(full_name) ON DELETE CASCADE,
                 workflow_name TEXT NOT NULL, branch TEXT NOT NULL, actor TEXT NOT NULL,
                 status TEXT NOT NULL, conclusion TEXT, commit_sha TEXT NOT NULL,
                 commit_subject TEXT NOT NULL, html_url TEXT NOT NULL, started_at TEXT NOT NULL);
             INSERT INTO repos (full_name, default_branch, etag)
                 VALUES ('a/one', 'main', 'W/\"stale\"');",
        )
        .unwrap();
    }

    let s = Store::open(&path).unwrap();
    // The ETag is cleared so the next cycle refetches and backfills workflow_id.
    assert_eq!(s.repo_etag("a/one").unwrap(), None);
    // And the new column exists.
    s.upsert_run(&run_full(
        1,
        "a/one",
        "W",
        "completed",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    assert_eq!(
        s.repo_summaries(false).unwrap()[0].workflows[0].workflow_path,
        Some(".github/workflows/lint.yml".to_string())
    );

    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_backed_store_uses_wal_so_two_processes_can_share_it() {
    let dir = std::env::temp_dir().join(format!("ghwd-wal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("runs.db");

    let s = Store::open(&path).unwrap();
    s.upsert_repo("a/one", "main").unwrap();

    // A second handle, as a second process would have.
    let other = Store::open(&path).unwrap();
    other
        .upsert_run(&run_full(
            1,
            "a/one",
            "W",
            "completed",
            Some("success"),
            "2026-08-04T09:00:00Z",
        ))
        .unwrap();
    assert_eq!(s.repo_summaries(false).unwrap().len(), 1);

    // WAL leaves its sidecar next to the database; its absence would mean the
    // pragma silently did not take.
    assert!(
        path.with_extension("db-wal").exists(),
        "expected a -wal file beside {}",
        path.display()
    );

    drop(s);
    drop(other);
    let _ = std::fs::remove_dir_all(&dir);
}
