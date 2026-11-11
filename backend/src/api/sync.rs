use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;

use super::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::database;
use crate::garmin::{GarminClient, GarminError};
use crate::models;
use crate::models::{SyncState, SyncStatus};
use crate::sync::{HealthRange, Report, sync_all};

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
    let status = start_sync_of(&state, &user)?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

/// Starts a sync of the account in the background, unless one is running.
fn start_sync_of(state: &AppState, user: &models::User) -> Result<SyncStatus, ApiError> {
    let Some(sealed) = &user.garmin_api_key else {
        return Err(ApiError::Conflict(
            "No Garmin API key is saved. Add one in the settings",
        ));
    };
    let api_key = state.auth.secret_box.open(sealed)?;

    let status = SyncStatus::running(&Report::default());
    {
        let mut statuses = state.sync.lock().unwrap();
        if statuses
            .get(&user.id)
            .is_some_and(|current| matches!(current.state, SyncState::Running))
        {
            return Err(ApiError::Conflict("a sync is already running"));
        }
        statuses.insert(user.id, status.clone());
    }

    tokio::spawn(run_sync(state.clone(), user.id, api_key));
    Ok(status)
}

/// Starts a sync of every account with a Garmin key every `interval`, so that
/// new activities show up without anyone selecting Sync.
pub async fn sync_periodically(state: AppState, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    // The first tick is immediate. Skipping it leaves a restart time to settle.
    ticks.tick().await;
    loop {
        ticks.tick().await;
        let users = match database::users_with_keys(&state.pool).await {
            Ok(users) => users,
            Err(error) => {
                tracing::error!("{error:#}");
                continue;
            }
        };
        for user in users {
            match start_sync_of(&state, &user) {
                Ok(_) => tracing::info!(user_id = user.id, "started an automatic sync"),
                Err(ApiError::Conflict(_)) => {}
                Err(error) => tracing::warn!(user_id = user.id, ?error, "no automatic sync"),
            }
        }
    }
}

async fn run_sync(state: AppState, user_id: i64, api_key: String) {
    let task = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut client = GarminClient::new(&state.garmin.base_url, &api_key)?
                .with_retry_delay(state.garmin.retry_delay);
            let set_status = |status: SyncStatus| {
                state.sync.lock().unwrap().insert(user_id, status);
            };
            sync_all(
                &state.pool,
                &state.files,
                &mut client,
                user_id,
                HealthRange {
                    today: chrono::Local::now().date_naive(),
                    history_days: state.garmin.health_history_days,
                },
                state.garmin.download_pause,
                |report| set_status(SyncStatus::running(report)),
            )
            .await
        })
    };

    state
        .sync_tasks
        .lock()
        .unwrap()
        .insert(user_id, task.abort_handle());
    // Awaits a separate task, so that a panic cannot leave the state at running.
    let result = task.await;
    state.sync_tasks.lock().unwrap().remove(&user_id);
    // Before the status changes, because that is what makes the page reload the map.
    state.forget_tracks(user_id);

    let saved = {
        let mut statuses = state.sync.lock().unwrap();
        let status = statuses.entry(user_id).or_default();
        match result {
            Ok(Ok(report)) => {
                *status = SyncStatus {
                    state: SyncState::Succeeded,
                    ..SyncStatus::running(&report)
                };
            }
            Ok(Err(error)) => {
                tracing::error!(%error, "sync failed");
                status.state = SyncState::Failed;
                // Database errors can name tables and queries, which are no use to the user.
                status.error = Some(match error {
                    GarminError::Database(_) => {
                        "the sync could not save the data. See the logs".to_owned()
                    }
                    error => error.to_string(),
                });
            }
            Err(error) if error.is_cancelled() => {
                tracing::info!(user_id, "sync cancelled");
                status.state = SyncState::Cancelled;
            }
            Err(error) => {
                tracing::error!(%error, "sync crashed");
                status.state = SyncState::Failed;
                status.error = Some("the sync crashed. See the logs".to_owned());
            }
        }
        status.finished_at = Some(Utc::now());
        serde_json::to_value(&*status).expect("the status is plain data")
    };
    if let Err(error) = database::save_last_sync(&state.pool, user_id, &saved).await {
        tracing::error!("{error:#}");
    }
}
