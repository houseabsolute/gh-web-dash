use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::config::Config;
use crate::inclusion::Override;
use crate::store::{ManagedRepo, RepoSummary, Store, WorkflowHistory};
use crate::sync::{apply_decision, SyncState, SyncStatus};

/// Runs per workflow in an expansion's history strip.
const HISTORY_PER_WORKFLOW: usize = 10;

const INDEX_HTML: &str = include_str!("assets/index.html");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_JS: &str = include_str!("assets/app.js");
const RENDER_JS: &str = include_str!("assets/render.js");
const REPOS_HTML: &str = include_str!("assets/repos.html");
const REPOS_JS: &str = include_str!("assets/repos.js");
/// The neutral fallback. The dashboard replaces it at runtime with one
/// coloured by whether anything is currently failing.
const FAVICON_SVG: &str = include_str!("assets/favicon.svg");

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub sync: SyncState,
    /// Needed to re-evaluate inclusion when the UI changes an override, so a
    /// toggle takes effect now rather than at the next hourly discovery.
    pub config: Arc<Config>,
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

#[derive(Debug, Deserialize)]
pub struct OverrideBody {
    pub repo: String,
    /// `"include"`, `"exclude"`, or null to fall back to the automatic rules.
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct ManagedResponse {
    pub repos: Vec<ManagedRepo>,
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
        .route("/favicon.svg", get(favicon))
        .route("/repos", get(repos_page))
        .route("/repos.js", get(repos_js))
        .route("/api/managed", get(managed))
        .route("/api/override", post(set_override))
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

async fn favicon() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], FAVICON_SVG)
}

async fn repos_page() -> Html<&'static str> {
    Html(REPOS_HTML)
}

async fn repos_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript")], REPOS_JS)
}

/// Every repository and why it is or is not polled.
async fn managed(State(state): State<AppState>) -> Result<Json<ManagedResponse>, StatusCode> {
    let repos = state.store.managed_repos().map_err(|e| {
        tracing::error!("managed repo query failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(ManagedResponse { repos }))
}

/// Record a manual include/exclude and re-evaluate that repository at once.
async fn set_override(
    State(state): State<AppState>,
    Json(body): Json<OverrideBody>,
) -> Result<Json<ManagedResponse>, StatusCode> {
    // Reject unknown values rather than storing something the rules ignore.
    let value = match body.value.as_deref() {
        None | Some("") => None,
        Some(v) => match Override::parse(v) {
            Some(o) => Some(o),
            None => return Err(StatusCode::BAD_REQUEST),
        },
    };

    state
        .store
        .set_repo_override(&body.repo, value.map(|o| o.as_str()))
        .map_err(|e| {
            tracing::warn!("override rejected: {e}");
            StatusCode::NOT_FOUND
        })?;
    apply_decision(&state.store, &state.config, &body.repo).map_err(|e| {
        tracing::error!("could not re-evaluate {}: {e}", body.repo);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let repos = state.store.managed_repos().map_err(|e| {
        tracing::error!("managed repo query failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(ManagedResponse { repos }))
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
