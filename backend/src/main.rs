use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use runmap::api::{self, AppState, GarminSettings};
use runmap::auth::{Auth, OidcSettings};
use runmap::db;
use runmap::garmin::DEFAULT_BASE_URL;
use runmap::secret::SecretBox;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Pause between track downloads, to stay gentle on Garmin's servers.
const DOWNLOAD_PAUSE: Duration = Duration::from_millis(500);
/// How far back health data goes at least, for accounts with few activities.
const HEALTH_HISTORY_DAYS: u64 = 365;

/// Saves Garmin activities and serves them as a heatmap.
#[derive(Parser)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "BIND_ADDRESS", default_value = "0.0.0.0:3000")]
    bind: SocketAddr,
    /// The address people open the app at, where the provider sends them back to.
    #[arg(long, env = "PUBLIC_URL", default_value = "http://localhost:8080")]
    public_url: String,
    /// The OpenID Connect issuer, like `https://auth.example.com/application/o/runmap/`.
    #[arg(long, env = "OIDC_ISSUER")]
    oidc_issuer: Option<String>,
    #[arg(long, env = "OIDC_CLIENT_ID")]
    oidc_client_id: Option<String>,
    /// A PEM file with certificates to trust for the provider, like a local test CA.
    #[arg(long, env = "OIDC_CA_CERT")]
    oidc_ca_cert: Option<PathBuf>,
}

/// Secrets are not command-line options: those end up in the shell history.
fn secret_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

    let secret_box = SecretBox::new(&secret_env("APP_SECRET").unwrap_or_default())?;
    let extra_roots = match &cli.oidc_ca_cert {
        Some(path) => {
            let pem = std::fs::read(path)
                .with_context(|| format!("could not read OIDC_CA_CERT {}", path.display()))?;
            reqwest::Certificate::from_pem_bundle(&pem).context("invalid OIDC_CA_CERT")?
        }
        None => Vec::new(),
    };
    let oidc = match (cli.oidc_issuer, cli.oidc_client_id) {
        (Some(issuer), Some(client_id)) => Some(OidcSettings {
            issuer,
            client_id,
            client_secret: secret_env("OIDC_CLIENT_SECRET"),
            public_url: cli.public_url,
            extra_roots,
        }),
        _ => {
            tracing::warn!("OIDC_ISSUER or OIDC_CLIENT_ID is not set, so nobody can sign in");
            None
        }
    };
    let state = AppState::new(
        pool,
        GarminSettings {
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: secret_env("GARMIN_PERSONAL_API_KEY"),
            download_pause: DOWNLOAD_PAUSE,
            health_history_days: HEALTH_HISTORY_DAYS,
        },
        Auth::new(oidc, secret_box),
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
