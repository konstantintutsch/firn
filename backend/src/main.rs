use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use runmap::api::{self, AppState, GarminSettings};
use runmap::auth::{Auth, OidcSettings};
use runmap::db;
use runmap::files::FileStore;
use runmap::garmin::DEFAULT_BASE_URL;
use runmap::secret::SecretBox;
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

/// Saves Garmin activities and serves them as a heatmap.
#[derive(Parser)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,
    /// Where original files and photos are kept.
    #[arg(long, env = "DATA_DIR", default_value = "/data")]
    data_dir: PathBuf,
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
    /// Where the Garmin Personal API is. Only changed to try Runmap with made-up data.
    #[arg(long, env = "GARMIN_BASE_URL", default_value = DEFAULT_BASE_URL)]
    garmin_base_url: String,
    /// Pause between Garmin requests in milliseconds, to stay gentle on Garmin's servers.
    #[arg(long, env = "DOWNLOAD_PAUSE_MS", default_value_t = 500)]
    download_pause_ms: u64,
    /// How far back health data goes at least, for accounts with few activities.
    #[arg(long, env = "HEALTH_HISTORY_DAYS", default_value_t = 365)]
    health_history_days: u64,
    /// Serves the API documentation at /api/docs.
    #[arg(long, env = "API_DOCS", default_value_t = true, action = clap::ArgAction::Set)]
    api_docs: bool,
    /// Logs as JSON lines instead of text, for log collectors.
    #[arg(long, env = "LOG_JSON", default_value_t = false, action = clap::ArgAction::Set)]
    log_json: bool,
    /// Syncs every account with a Garmin key every this many hours. 0 turns it off.
    #[arg(long, env = "SYNC_INTERVAL_HOURS", default_value_t = 0)]
    sync_interval_hours: u64,
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
    // `runmap healthcheck` asks the running backend, for the container's health
    // check, since the image has no curl.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck().await;
    }
    let cli = Cli::parse();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if cli.log_json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let pool = db::connect(&cli.database_url)?;
    db::migrate(&pool).await?;
    let files = FileStore::new(&cli.data_dir);
    db::rename_activity_folders(&pool, &files).await?;
    db::move_files_to_store(&pool, &files).await?;
    db::backfill_vo2_max(&pool, &files).await?;

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
        files,
        GarminSettings {
            base_url: cli.garmin_base_url,
            api_key: secret_env("GARMIN_PERSONAL_API_KEY"),
            download_pause: Duration::from_millis(cli.download_pause_ms),
            retry_delay: Duration::from_secs(2),
            health_history_days: cli.health_history_days,
        },
        Auth::new(oidc, secret_box),
    );

    if cli.sync_interval_hours > 0 {
        let interval = Duration::from_secs(cli.sync_interval_hours * 3600);
        tokio::spawn(api::sync_periodically(state.clone(), interval));
    }

    let listener = TcpListener::bind(cli.bind).await?;
    tracing::info!(bind = %cli.bind, "listening");
    axum::serve(listener, api::router_with(state, cli.api_docs))
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

async fn healthcheck() -> Result<()> {
    let bind: SocketAddr = std::env::var("BIND_ADDRESS")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_owned())
        .parse()
        .context("invalid BIND_ADDRESS")?;
    let url = format!("http://127.0.0.1:{}/api/health", bind.port());
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    anyhow::ensure!(
        response.status().is_success(),
        "{url} answered {}",
        response.status()
    );
    Ok(())
}
