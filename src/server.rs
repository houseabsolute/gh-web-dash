use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::store::{RepoSummary, Store, WorkflowHistory};
use crate::sync::{SyncState, SyncStatus};

/// Runs per workflow in an expansion's history strip.
const HISTORY_PER_WORKFLOW: usize = 10;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_JS: &str = include_str!("assets/app.js");
const RENDER_JS: &str = include_str!("assets/render.js");

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub sync: SyncState,
    /// Sending on this asks the poll loop to run a cycle immediately.
    pub trigger: Sender<()>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ReposParams {
    #[serde(default)]
    pub failures_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    pub repo: String,
}

#[derive(Serialize)]
pub struct ReposResponse {
    pub repos: Vec<RepoSummary>,
}

#[derive(Serialize)]
pub struct HistoryResponse {
    pub workflows: Vec<WorkflowHistory>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(app_css))
        .route("/app.js", get(app_js))
        .route("/render.js", get(render_js))
        .route("/api/repos", get(repos))
        .route("/api/history", get(history))
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

async fn render_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript")], RENDER_JS)
}

async fn repos(
    State(state): State<AppState>,
    Query(params): Query<ReposParams>,
) -> Result<Json<ReposResponse>, StatusCode> {
    let repos = state
        .store
        .repo_summaries(params.failures_only.unwrap_or(false))
        .map_err(|e| {
            tracing::error!("repo summary query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(ReposResponse { repos }))
}

async fn history(
    State(state): State<AppState>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<HistoryResponse>, StatusCode> {
    let workflows = state
        .store
        .repo_history(&params.repo, HISTORY_PER_WORKFLOW)
        .map_err(|e| {
            tracing::error!("history query failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(HistoryResponse { workflows }))
}

async fn status(State(state): State<AppState>) -> Json<SyncStatus> {
    Json(state.sync.snapshot())
}

async fn trigger_sync(State(state): State<AppState>) -> StatusCode {
    // A full channel means a sync is already queued — that is success, not an error.
    let _ = state.trigger.try_send(());
    StatusCode::ACCEPTED
}
