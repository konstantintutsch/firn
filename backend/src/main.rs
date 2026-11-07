use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use runmap::garmin::{self, Endpoints, GarminClient};
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
    /// Sign in to Garmin and save the session. The password is not stored
    Login,
    /// Save all runs that are not saved yet
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
        Command::Login => {
            let email = prompt("Garmin email: ")?;
            let password = rpassword::prompt_password("Garmin password: ")?;
            let tokens = garmin::login(&Endpoints::default(), &email, &password, || {
                prompt("MFA code: ")
            })
            .await?;
            db::save_tokens(&pool, &tokens).await?;
            println!("Signed in. Run `just sync` to save your runs.");
        }
        Command::Sync => {
            let mut client =
                GarminClient::from_saved_tokens(pool.clone(), Endpoints::default()).await?;
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

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read input")?;
    Ok(line.trim().to_owned())
}
