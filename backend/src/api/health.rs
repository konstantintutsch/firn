use axum::extract::State;
use axum::http::StatusCode;

use super::AppState;
use crate::database;

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
    match database::ping(&state.pool).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!("health check failed: {error:#}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}
