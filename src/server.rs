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
    let workflows = state.store.workflow_names_for_query(&q).unwrap_or_default();
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
