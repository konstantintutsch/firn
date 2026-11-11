//! Slows down guessing at sign-ins, with a limit for each client.
use std::sync::Arc;
use std::time::Duration;

use axum::http::header::RETRY_AFTER;
use axum::response::IntoResponse;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};
use utoipa_axum::router::OpenApiRouter;

use crate::endpoints::ApiError;
use crate::state::AppState;

/// Requests that a client may send at once.
const BURST: u32 = 20;
/// One more request every this long after that, which is 10 a minute.
const REFILL: Duration = Duration::from_secs(6);
/// How often clients that have their full limit again are forgotten.
const CLEANUP: Duration = Duration::from_secs(60);

/// Limits the requests of each client to these routes. Must run inside Tokio.
///
/// The client's address comes from the X-Forwarded-For header, which the
/// frontend sets to the one address it trusts, or else from the connection.
pub fn limited(routes: OpenApiRouter<AppState>) -> OpenApiRouter<AppState> {
    let config = GovernorConfigBuilder::default()
        .key_extractor(SmartIpKeyExtractor)
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
