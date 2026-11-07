use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use runmap::api::{self, AppState, GarminSettings};
use runmap::db;
use runmap::garmin::DEFAULT_BASE_URL;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Pause between track downloads, to stay gentle on Garmin's servers.
const DOWNLOAD_PAUSE: Duration = Duration::from_millis(500);

/// Saves Garmin activities and serves them as a heatmap.
#[derive(Parser)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "BIND_ADDRESS", default_value = "0.0.0.0:3000")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let pool = db::connect(&cli.database_url)?;
    db::migrate(&pool).await?;

    // Not a command-line option: those end up in the shell history.
    let api_key = std::env::var("GARMIN_PERSONAL_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty());
    let state = AppState::new(
        pool,
        GarminSettings {
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key,
            download_pause: DOWNLOAD_PAUSE,
        },
    );

    let listener = TcpListener::bind(cli.bind).await?;
    tracing::info!(bind = %cli.bind, "listening");
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Docker stops containers with SIGTERM, and a terminal with SIGINT.
async fn shutdown_signal() {
    let mut terminate = signal(SignalKind::terminate()).expect("could not listen for SIGTERM");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
