//! The HTTP API. Each cluster of paths has a module with a router of its own,
//! which takes the path and the method of each endpoint from its `utoipa::path`.

use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;

use crate::state::AppState;

mod activities;
mod activity_types;
mod auth;
mod error;
mod filter;
mod health;
mod heatmap;
mod me;
mod routes;
mod statistics;
mod stats;
mod sync;
mod training_blocks;
mod transfer;

pub use error::{ApiError, require_header};

/// What the API documentation says besides its paths, which the routers add.
#[derive(OpenApi)]
#[openapi(info(
    title = "firn",
    description = "Heatmap of activities saved from Garmin Connect"
))]
struct ApiDoc;

/// All routes, with the API documentation at /api/docs.
pub fn router(state: AppState) -> Router {
    router_with(state, true)
}

/// All routes. With `docs`, also the API documentation at /api/docs.
pub fn router_with(state: AppState, docs: bool) -> Router {
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(activities::router())
        .merge(activity_types::router())
        .merge(auth::router())
        .merge(health::router())
        .merge(heatmap::router())
        .merge(me::router())
        .merge(routes::router())
        .merge(statistics::router())
        .merge(stats::router())
        .merge(sync::router())
        .merge(training_blocks::router())
        .merge(transfer::router())
        .split_for_parts();
    let router = router
        .with_state(state)
        // Logs each request with its status and duration, at debug level.
        .layer(tower_http::trace::TraceLayer::new_for_http());
    if docs {
        router.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", api))
    } else {
        router
    }
}
