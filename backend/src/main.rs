use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use runmap::garmin::{DEFAULT_BASE_URL, GarminClient};
use runmap::{api, db, sync};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Pause between track downloads, to stay gentle on Garmin's servers.
const DOWNLOAD_PAUSE: Duration = Duration::from_millis(500);

#[derive(Parser)]
#[command(about = "Saves Garmin runs and serves them as a heatmap")]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the HTTP API
    Serve {
        #[arg(long, env = "BIND_ADDRESS", default_value = "0.0.0.0:3000")]
        bind: SocketAddr,
    },
    /// Save all runs that are not saved yet. Reads GARMIN_PERSONAL_API_KEY
    Sync,
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

    match cli.command {
        Command::Serve { bind } => {
            let listener = TcpListener::bind(bind).await?;
            tracing::info!(%bind, "listening");
            axum::serve(listener, api::router(pool))
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }
        Command::Sync => {
            // Not a command-line option: those end up in the shell history.
            let api_key = std::env::var("GARMIN_PERSONAL_API_KEY").unwrap_or_default();
            if api_key.trim().is_empty() {
                bail!(
                    "GARMIN_PERSONAL_API_KEY is not set. Create a key at \
                     https://connect.garmin.com/app/settings/personalApiToken \
                     and add it to .env"
                );
            }
            let mut client = GarminClient::new(DEFAULT_BASE_URL, &api_key)?;
            let report = sync::sync_runs(&pool, &mut client, DOWNLOAD_PAUSE).await?;
            println!(
                "{} runs listed, {} tracks downloaded, {} failed",
                report.listed, report.downloaded, report.failed
            );
            if report.failed > 0 {
                bail!(
                    "{} tracks failed. Run `just sync` to retry them",
                    report.failed
                );
            }
        }
    }
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
