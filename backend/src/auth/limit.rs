//! Slows down guessing at sign-ins, with a limit for each client.
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::http::Request;
use axum::http::header::RETRY_AFTER;
use axum::response::IntoResponse;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

use crate::api::{ApiError, AppState};

/// Requests that a client may send at once.
const BURST: u32 = 20;
/// One more request every this long after that, which is 10 a minute.
const REFILL: Duration = Duration::from_secs(6);
/// How often clients that have their full limit again are forgotten.
const CLEANUP: Duration = Duration::from_secs(60);

/// The address of the client. The frontend puts it into the X-Forwarded-For
/// header. Only the last address counts, which the frontend added itself:
/// those before it come from the client and could be made up.
#[derive(Clone)]
struct ClientAddress;

impl KeyExtractor for ClientAddress {
    type Key = IpAddr;

    fn extract<T>(&self, request: &Request<T>) -> Result<IpAddr, GovernorError> {
        let forwarded = request
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .next_back()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|address| address.trim().parse().ok());
        let peer = || {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(address)| address.ip())
        };
        // Requests without any address share one limit.
        Ok(forwarded
            .or_else(peer)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    }
}

/// Limits the requests of each client to these routes. Must run inside Tokio.
pub fn limited(routes: Router<AppState>) -> Router<AppState> {
    let config = GovernorConfigBuilder::default()
        .key_extractor(ClientAddress)
        .period(REFILL)
        .burst_size(BURST)
        .finish()
        .expect("the limit is not zero");

    // Ends with the router, which holds the only lasting reference.
    let limiter = Arc::downgrade(config.limiter());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CLEANUP).await;
            let Some(limiter) = limiter.upgrade() else {
                break;
            };
            limiter.retain_recent();
        }
    });

    routes.layer(GovernorLayer::new(config).error_handler(|error| {
        match error {
            GovernorError::TooManyRequests { wait_time, .. } => (
                [(RETRY_AFTER, wait_time.to_string())],
                ApiError::TooManyRequests("too many requests. Try again in a minute"),
            )
                .into_response(),
            error => error.into(),
        }
    }))
}
