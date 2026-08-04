use gh_web_dash::store::{Health, RepoSummary, RunQuery, Store, StoredRun};

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

    s.upsert_run(&run(1, "autarch/precious", None, "2026-08-04T10:00:00Z"))
        .unwrap();
    s.upsert_run(&run(1, "autarch/precious", None, "2026-08-04T10:00:00Z"))
        .unwrap();
    assert_eq!(s.recent_runs(&RunQuery::default()).unwrap().len(), 1);

    s.upsert_run(&run(
        1,
        "autarch/precious",
        Some("failure"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    let rows = s.recent_runs(&RunQuery::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].conclusion.as_deref(), Some("failure"));
}

#[test]
fn recent_runs_are_newest_first() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_run(&run(
        1,
        "autarch/precious",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run(
        2,
        "autarch/precious",
        Some("success"),
        "2026-08-04T11:00:00Z",
    ))
    .unwrap();
    let rows = s.recent_runs(&RunQuery::default()).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 1]);
}

#[test]
fn failures_only_excludes_success_and_in_progress() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_run(&run(
        1,
        "autarch/precious",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run(
        2,
        "autarch/precious",
        Some("failure"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run(3, "autarch/precious", None, "2026-08-04T11:00:00Z"))
        .unwrap();

    let q = RunQuery {
        failures_only: true,
        ..RunQuery::default()
    };
    let rows = s.recent_runs(&q).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
}

#[test]
fn limit_caps_results() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    for i in 1..=5 {
        s.upsert_run(&run(
            i,
            "autarch/precious",
            Some("success"),
            "2026-08-04T09:00:00Z",
        ))
        .unwrap();
    }
    let q = RunQuery {
        limit: 2,
        ..RunQuery::default()
    };
    assert_eq!(s.recent_runs(&q).unwrap().len(), 2);
}

#[test]
fn prune_removes_only_old_runs() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_run(&run(
        1,
        "autarch/precious",
        Some("success"),
        "2020-01-01T00:00:00Z",
    ))
    .unwrap();
    s.upsert_run(&run(
        2,
        "autarch/precious",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();

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

#[test]
fn ignored_repos_runs_are_excluded_from_recent_runs_and_workflow_names() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    s.upsert_repo("autarch/noisy", "main").unwrap();
    s.set_repo_ignored("autarch/noisy", true).unwrap();

    s.upsert_run(&run(
        1,
        "autarch/precious",
        Some("success"),
        "2026-08-04T09:00:00Z",
    ))
    .unwrap();
    let mut noisy_run = run(2, "autarch/noisy", Some("success"), "2026-08-04T10:00:00Z");
    noisy_run.workflow_name = "noisy.yml".to_string();
    s.upsert_run(&noisy_run).unwrap();

    let rows = s.recent_runs(&RunQuery::default()).unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);

    assert_eq!(
        s.workflow_names_for_query(&RunQuery::default()).unwrap(),
        vec!["test.yml".to_string()]
    );
}

#[test]
fn workflow_names_for_query_ignores_the_workflow_filter_itself() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    let mut failing = run(
        1,
        "autarch/precious",
        Some("failure"),
        "2026-08-04T09:00:00Z",
    );
    failing.workflow_name = "test.yml".to_string();
    s.upsert_run(&failing).unwrap();
    let mut passing = run(
        2,
        "autarch/precious",
        Some("success"),
        "2026-08-04T10:00:00Z",
    );
    passing.workflow_name = "release.yml".to_string();
    s.upsert_run(&passing).unwrap();

    // failures_only=true, only test.yml has a failing run.
    let q = RunQuery {
        failures_only: true,
        ..RunQuery::default()
    };
    assert_eq!(
        s.workflow_names_for_query(&q).unwrap(),
        vec!["test.yml".to_string()]
    );

    // Selecting a workflow must not shrink the chip set to just itself:
    // the other filters (none here) still admit both workflows.
    let q2 = RunQuery {
        workflow: Some("test.yml".to_string()),
        ..RunQuery::default()
    };
    assert_eq!(
        s.workflow_names_for_query(&q2).unwrap(),
        vec!["release.yml".to_string(), "test.yml".to_string()]
    );
}

#[test]
fn workflow_names_are_distinct_and_sorted() {
    let s = Store::open_in_memory().unwrap();
    s.upsert_repo("autarch/precious", "main").unwrap();
    let mut r = run(
        1,
        "autarch/precious",
        Some("success"),
        "2026-08-04T09:00:00Z",
    );
    r.workflow_name = "release.yml".to_string();
    s.upsert_run(&r).unwrap();
    s.upsert_run(&run(
        2,
        "autarch/precious",
        Some("success"),
        "2026-08-04T10:00:00Z",
    ))
    .unwrap();
    assert_eq!(
        s.workflow_names_for_query(&RunQuery::default()).unwrap(),
        vec!["release.yml".to_string(), "test.yml".to_string()]
    );
}

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
        conclusion: conclusion.map(|s| s.to_string()),
        commit_sha: "abc123".to_string(),
        commit_subject: "Do a thing".to_string(),
        html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
        started_at: started.to_string(),
    }
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
            i as i64 + 1,
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
