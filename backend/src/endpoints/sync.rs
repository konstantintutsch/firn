//! `/api/sync`: saving what is new on Garmin.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::{ApiError, require_header};
use crate::authentication::CurrentUser;
use crate::database;
use crate::models::{ErrorBody, SyncStatus};
use crate::state::AppState;
use crate::sync;

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(sync_status, start_sync, cancel_sync))
}

/// Progress of the account's current or last sync.
#[utoipa::path(
    get,
    path = "/api/sync",
    responses(
        (status = 200, body = SyncStatus),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn sync_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<SyncStatus>, ApiError> {
    if let Some(status) = state.sync.lock().unwrap().get(&user.id) {
        return Ok(Json(status.clone()));
    }
    let saved = database::last_sync(&state.pool, user.id)
        .await?
        // A status saved by an older version may not fit, which is no reason to fail.
        .and_then(|saved| serde_json::from_value(saved).ok());
    Ok(Json(saved.unwrap_or_default()))
}

/// Stops the account's running sync. What it saved so far stays, and the next
/// sync continues from there.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/sync",
    responses(
        (status = 204, description = "The sync stopped"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "No sync is running", body = ErrorBody),
    )
)]
pub(super) async fn cancel_sync(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    if state.cancel_sync(user.id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::Conflict("no sync is running"))
    }
}

/// Starts saving everything from Garmin that is not saved yet, in the background:
/// activities with their files and details, then health data like sleep and HRV.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/sync",
    responses(
        (status = 202, description = "The sync started", body = SyncStatus),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "A sync is running or no API key is set", body = ErrorBody),
    )
)]
pub(super) async fn start_sync(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<SyncStatus>), ApiError> {
    require_header(&headers)?;
    let status = sync::start_sync_of(&state, &user)?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}
