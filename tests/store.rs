use gh_web_dash::store::{RunQuery, Store, StoredRun};

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
