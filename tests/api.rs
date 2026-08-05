use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use gh_web_dash::config::Config;
use gh_web_dash::server::{router, AppState};
use gh_web_dash::store::{Store, StoredRun};
use gh_web_dash::sync::SyncState;

fn run(
    id: i64,
    repo: &str,
    workflow: &str,
    status: &str,
    conclusion: Option<&str>,
    started: &str,
) -> StoredRun {
    StoredRun {
        id,
        repo_full_name: repo.into(),
        workflow_name: workflow.into(),
        branch: "main".into(),
        actor: "autarch".into(),
        status: status.into(),
        conclusion: conclusion.map(std::string::ToString::to_string),
        commit_sha: "abc123".into(),
        commit_subject: "Do a thing".into(),
        html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
        started_at: started.into(),
        workflow_path: Some(".github/workflows/lint.yml".to_string()),
    }
}

fn seeded_state() -> AppState {
    let store = Store::open_in_memory().unwrap();
    for repo in ["autarch/precious", "autarch/ubi"] {
        store.upsert_repo(repo, "main").unwrap();
    }
    // precious: two workflows, one failing, newest activity
    store
        .upsert_run(&run(
            1,
            "autarch/precious",
            "Run tests",
            "completed",
            Some("success"),
            "2026-08-04T09:00:00Z",
        ))
        .unwrap();
    store
        .upsert_run(&run(
            2,
            "autarch/precious",
            "Run tests",
            "completed",
            Some("failure"),
            "2026-08-04T11:00:00Z",
        ))
        .unwrap();
    store
        .upsert_run(&run(
            3,
            "autarch/precious",
            "Lint",
            "completed",
            Some("success"),
            "2026-08-04T10:00:00Z",
        ))
        .unwrap();
    // ubi: one workflow, healthy, older
    store
        .upsert_run(&run(
            4,
            "autarch/ubi",
            "Run tests",
            "completed",
            Some("success"),
            "2026-08-04T08:00:00Z",
        ))
        .unwrap();

    AppState {
        store,
        sync: SyncState::default(),
        config: std::sync::Arc::new(Config::from_toml("").unwrap()),
        trigger: tokio::sync::mpsc::channel(1).0,
    }
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
async fn render_js_is_served() {
    let resp = router(seeded_state())
        .oneshot(
            Request::builder()
                .uri("/render.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn repos_are_sorted_by_most_recent_activity() {
    let v = get_json("/api/repos").await;
    let names: Vec<&str> = v["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["autarch/precious", "autarch/ubi"]);
}

#[tokio::test]
async fn repo_carries_workflows_and_rolled_up_health() {
    let v = get_json("/api/repos").await;
    let precious = &v["repos"][0];
    assert_eq!(precious["health"], "failure");
    let wfs = precious["workflows"].as_array().unwrap();
    assert_eq!(wfs.len(), 2);
    let names: Vec<&str> = wfs
        .iter()
        .map(|w| w["workflow_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Lint", "Run tests"]);
    let tests = wfs
        .iter()
        .find(|w| w["workflow_name"] == "Run tests")
        .unwrap();
    assert_eq!(tests["health"], "failure");
    assert_eq!(tests["branch"], "main");
}

#[tokio::test]
async fn failures_only_filters_repos() {
    let v = get_json("/api/repos?failures_only=true").await;
    let names: Vec<&str> = v["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["autarch/precious"]);
}

#[tokio::test]
async fn history_returns_each_workflow_newest_first() {
    let v = get_json("/api/history?repo=autarch/precious").await;
    let wfs = v["workflows"].as_array().unwrap();
    assert_eq!(wfs.len(), 2);
    let tests = wfs
        .iter()
        .find(|w| w["workflow_name"] == "Run tests")
        .unwrap();
    let ids: Vec<i64> = tests["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![2, 1]);
}

#[tokio::test]
async fn history_requires_a_repo_parameter() {
    let resp = router(seeded_state())
        .oneshot(
            Request::builder()
                .uri("/api/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_of_an_unknown_repo_is_empty_not_an_error() {
    let v = get_json("/api/history?repo=autarch/nope").await;
    assert!(v["workflows"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn old_runs_endpoint_is_gone() {
    let resp = router(seeded_state())
        .oneshot(
            Request::builder()
                .uri("/api/runs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn status_reports_sync_state() {
    let v = get_json("/api/status").await;
    assert!(v.get("error_count").is_some());
    assert!(v.get("last_success").is_some());
    // Present but null when no cycle is running — the page keys off this.
    assert!(v.get("progress").is_some());
    assert!(v["progress"].is_null());
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

async fn post_json(
    path: &str,
    body: serde_json::Value,
    state: AppState,
) -> (StatusCode, serde_json::Value) {
    let resp = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn managed_lists_every_repo_included_or_not() {
    let state = seeded_state();
    state.store.upsert_repo("autarch/quiet", "main").unwrap();
    state
        .store
        .set_repo_decision("autarch/quiet", false, Some("stale"))
        .unwrap();

    let resp = router(state)
        .oneshot(
            Request::builder()
                .uri("/api/managed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let repos = v["repos"].as_array().unwrap();

    // Skipped repos appear too — the point of the page is explaining absences.
    let names: Vec<&str> = repos
        .iter()
        .map(|r| r["full_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"autarch/quiet"), "got: {names:?}");

    let quiet = repos
        .iter()
        .find(|r| r["full_name"] == "autarch/quiet")
        .unwrap();
    assert_eq!(quiet["included"], false);
    assert_eq!(quiet["skip_reason"], "stale");
    assert_eq!(quiet["run_count"], 0);

    let precious = repos
        .iter()
        .find(|r| r["full_name"] == "autarch/precious")
        .unwrap();
    assert_eq!(precious["included"], true);
    assert_eq!(precious["run_count"], 3);
}

#[tokio::test]
async fn muting_takes_effect_immediately() {
    let state = seeded_state();
    let (status, v) = post_json(
        "/api/override",
        serde_json::json!({"repo": "autarch/precious", "value": "exclude"}),
        state.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The response already reflects the re-evaluation — no waiting for discovery.
    let precious = v["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["full_name"] == "autarch/precious")
        .unwrap()
        .clone();
    assert_eq!(precious["included"], false);
    assert_eq!(precious["skip_reason"], "muted");
    assert_eq!(precious["user_override"], "exclude");

    // And it is really gone from the dashboard, not just relabelled.
    assert!(state
        .store
        .repo_summaries(false)
        .unwrap()
        .iter()
        .all(|r| r.full_name != "autarch/precious"));
}

#[tokio::test]
async fn clearing_an_override_restores_the_automatic_answer() {
    let state = seeded_state();
    post_json(
        "/api/override",
        serde_json::json!({"repo": "autarch/precious", "value": "exclude"}),
        state.clone(),
    )
    .await;
    let (status, v) = post_json(
        "/api/override",
        serde_json::json!({"repo": "autarch/precious", "value": null}),
        state.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let precious = v["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["full_name"] == "autarch/precious")
        .unwrap()
        .clone();
    assert_eq!(precious["included"], true);
    assert!(precious["user_override"].is_null());
}

#[tokio::test]
async fn an_unknown_override_value_is_rejected() {
    let (status, _) = post_json(
        "/api/override",
        serde_json::json!({"repo": "autarch/precious", "value": "maybe"}),
        seeded_state(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn overriding_an_unknown_repo_is_not_found() {
    let (status, _) = post_json(
        "/api/override",
        serde_json::json!({"repo": "autarch/never-heard-of-it", "value": "exclude"}),
        seeded_state(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn repos_page_and_script_are_served() {
    for path in ["/repos", "/repos.js"] {
        let resp = router(seeded_state())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    }
}
