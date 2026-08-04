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
async fn runs_are_newest_first() {
    let v = get_json("/api/runs").await;
    let ids: Vec<i64> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![3, 2, 1]);
}

#[tokio::test]
async fn workflow_names_are_included() {
    let v = get_json("/api/runs").await;
    let names: Vec<&str> = v["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["release.yml", "test.yml"]);
}

#[tokio::test]
async fn failures_only_filters() {
    let v = get_json("/api/runs?failures_only=true").await;
    let ids: Vec<i64> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![2]);
}

#[tokio::test]
async fn workflow_filter_applies() {
    let v = get_json("/api/runs?workflow=release.yml").await;
    let ids: Vec<i64> = v["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
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
