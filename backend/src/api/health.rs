use axum::extract::State;
use axum::http::StatusCode;

use super::AppState;
use crate::db::{self};

/// Whether the backend runs and reaches its database, for container health checks.
#[utoipa::path(
    get,
    path = "/api/health",
    responses(
        (status = 204, description = "Healthy"),
        (status = 503, description = "The database cannot be reached"),
    )
)]
pub(super) async fn health(State(state): State<AppState>) -> StatusCode {
    match db::ping(&state.pool).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!("health check failed: {error:#}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
