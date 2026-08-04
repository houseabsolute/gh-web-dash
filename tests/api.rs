use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

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
        conclusion: conclusion.map(|s| s.to_string()),
        commit_sha: "abc123".into(),
        commit_subject: "Do a thing".into(),
        html_url: format!("https://github.com/{repo}/actions/runs/{id}"),
        started_at: started.into(),
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
