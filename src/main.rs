use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::mpsc;

use gh_web_dash::config::{default_config_path, default_db_path, Config};
use gh_web_dash::github::{Client, GITHUB_API};
use gh_web_dash::server::{router, AppState};
use gh_web_dash::store::Store;
use gh_web_dash::sync::{
    discover_repos, effective_interval, sync_runs, SyncState, DISCOVERY_INTERVAL_SECS,
};

#[derive(Parser)]
#[command(about = "A local dashboard of recent GitHub Actions runs")]
struct Args {
    /// Do not open a browser on startup.
    #[arg(long)]
    no_open: bool,
    /// Path to the config file.
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gh_web_dash=info".into()),
        )
        .init();

    let args = Args::parse();

    // Startup failures are fatal and must name the fix.
    let config_path = match args.config {
        Some(p) => p,
        None => default_config_path()?,
    };
    let cfg = Config::load_or_create(&config_path)
        .with_context(|| format!("failed to load config from {}", config_path.display()))?;
    let token = gh_web_dash::auth::resolve_token().await?;
    let store = Store::open(&default_db_path()?)?;
    let client = Client::new(GITHUB_API.to_string(), token)?;

    let current_user = client
        .current_user()
        .await
        .context("could not identify you to GitHub — is the token valid?")?;
    tracing::info!("authenticated as {current_user}");

    let sync_state = SyncState::default();
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<()>(1);

    // Background poll loop. Discovery runs on its own slower cadence.
    {
        let client = client.clone();
        let store = store.clone();
        let sync_state = sync_state.clone();
        let cfg = cfg.clone();
        let user = current_user.clone();
        tokio::spawn(async move {
            let mut last_discovery: Option<std::time::Instant> = None;
            loop {
                let due = last_discovery
                    .map(|t| t.elapsed().as_secs() >= DISCOVERY_INTERVAL_SECS)
                    .unwrap_or(true);
                if due {
                    match discover_repos(&client, &store, &cfg).await {
                        Ok(()) => last_discovery = Some(std::time::Instant::now()),
                        Err(e) => tracing::warn!("repository discovery failed: {e}"),
                    }
                }

                sync_runs(&client, &store, &sync_state, &user).await;

                let secs = effective_interval(
                    cfg.poll_interval_secs,
                    sync_state.snapshot().rate_limit_remaining,
                );
                sync_state.record_poll_interval(secs);
                // Wake early if the browser asked for a sync.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
                    _ = trigger_rx.recv() => {}
                }
            }
        });
    }

    let state = AppState {
        store,
        sync: sync_state,
        config: std::sync::Arc::new(cfg),
        trigger: trigger_tx,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("could not bind a local port")?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    println!("gh-web-dash listening on {url}");

    if !args.no_open {
        if let Err(e) = open::that_detached(&url) {
            tracing::warn!("could not open a browser ({e}) — visit {url}");
        }
    }

    axum::serve(listener, router(state))
        .await
        .context("server error")?;
    Ok(())
}
