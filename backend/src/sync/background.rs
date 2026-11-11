//! Syncs that run in the background, started by a request or by the clock.

use std::time::Duration;

use chrono::Utc;

use super::{HealthRange, Report, sync_all};
use crate::database;
use crate::endpoints::ApiError;
use crate::garmin::{GarminClient, GarminError};
use crate::models::{SyncState, SyncStatus, User};
use crate::state::AppState;

/// Starts a sync of the account in the background, unless one is running.
pub(crate) fn start_sync_of(state: &AppState, user: &User) -> Result<SyncStatus, ApiError> {
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
