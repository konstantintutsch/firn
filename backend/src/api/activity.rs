use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use uuid::Uuid;

use super::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::database;
use crate::garmin::{GarminClient, GarminError};
use crate::models::{ActivityDetail, ActivityItem, Samples};
use crate::sync::{self};
use crate::track::{self};

/// Garmin lists zones per sport, like RUNNING or CYCLING, and DEFAULT for the rest.
fn heart_rate_zones(document: &Value, activity_type: &str) -> Option<Vec<f64>> {
    let zones = document.as_array()?;
    let sport = if activity_type.contains("running") {
        "RUNNING"
    } else if activity_type.contains("cycling") || activity_type.contains("biking") {
        "CYCLING"
    } else {
        "DEFAULT"
    };
    let of_sport = |name: &str| {
        zones
            .iter()
            .find(|zone| zone.get("sport").and_then(Value::as_str) == Some(name))
    };
    let zone = of_sport(sport).or_else(|| of_sport("DEFAULT"))?;
    [
        "zone1Floor",
        "zone2Floor",
        "zone3Floor",
        "zone4Floor",
        "zone5Floor",
        "maxHeartRateUsed",
    ]
    .iter()
    .map(|key| zone.get(key).and_then(Value::as_f64))
    .collect()
}

/// One activity with all its metrics and what the device recorded along it.
#[utoipa::path(
    get,
    path = "/api/activities/{id}",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    responses(
        (status = 200, body = ActivityDetail),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such activity", body = ErrorBody),
    )
)]
pub(super) async fn activity(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ActivityDetail>, ApiError> {
    let (activity, summary) = database::activity_with_summary(&state.pool, user.id, id)
        .await?
        .ok_or(ApiError::NotFound("no such activity"))?;
    let (samples, original_format) = match state.files.original(id).await? {
        Some(zip) => tokio::task::spawn_blocking(move || {
            let format = track::original_file(&zip).ok().and_then(|(name, _)| {
                name.rsplit_once('.')
                    .map(|(_, extension)| extension.to_owned())
            });
            let samples = track::parse_samples(&zip).unwrap_or_else(|error| {
                // The file was read once already to save the track, so this should not happen.
                tracing::warn!(%id, "could not read the activity file: {error:#}");
                Vec::new()
            });
            (samples, format)
        })
        .await
        .map_err(anyhow::Error::from)?,
        None => (Vec::new(), None),
    };
    let bounds = samples.iter().filter_map(|sample| sample.position).fold(
        None,
        |bounds: Option<[f64; 4]>, point| {
            Some(match bounds {
                None => [point.lon, point.lat, point.lon, point.lat],
                Some([west, south, east, north]) => [
                    west.min(point.lon),
                    south.min(point.lat),
                    east.max(point.lon),
                    north.max(point.lat),
                ],
            })
        },
    );
    let images = database::image_ids(&state.pool, id)
        .await?
        .into_iter()
        .map(|image_id| format!("/api/activities/{id}/images/{image_id}"))
        .collect();
    let heart_rate_zones = database::document(&state.pool, user.id, "heart_rate_zones", "")
        .await?
        .and_then(|document| heart_rate_zones(&document, &activity.activity_type));
    let (previous_id, next_id) =
        database::neighbors(&state.pool, user.id, id, activity.started_at).await?;
    let result = database::race_results(&state.pool, &[id])
        .await?
        .remove(&id);
    Ok(Json(ActivityDetail {
        activity: ActivityItem::new(activity, bounds, result.clone()),
        summary,
        samples: Samples::from_samples(&samples),
        images,
        heart_rate_zones,
        original_format,
        result,
        previous_id,
        next_id,
    }))
}

/// Downloads an activity again from Garmin: its file, track, details, workouts
/// and photos. Waits until it is done, which takes a few seconds.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/activities/{id}/sync",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    responses(
        (status = 204, description = "The activity is up to date"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such activity", body = ErrorBody),
        (status = 409, description = "A sync is running or no API key is set", body = ErrorBody),
        (status = 502, description = "Garmin failed", body = ErrorBody),
    )
)]
pub(super) async fn resync_activity(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let Some(sealed) = &user.garmin_api_key else {
        return Err(ApiError::Conflict(
            "No Garmin API key is saved. Add one in the settings",
        ));
    };
    // Both would write the same activity.
    if state.sync_tasks.lock().unwrap().contains_key(&user.id) {
        return Err(ApiError::Conflict("a sync is already running"));
    }
    let api_key = state.auth.secret_box.open(sealed)?;
    let mut client = GarminClient::new(&state.garmin.base_url, &api_key)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .with_retry_delay(state.garmin.retry_delay);
    let result = sync::resync_activity(&state.pool, &state.files, &mut client, user.id, id).await;
    state.forget_tracks(user.id);
    match result {
        Ok(Some(())) => Ok(StatusCode::NO_CONTENT),
        Ok(None) => Err(ApiError::NotFound("no such activity")),
        Err(GarminError::Database(error)) => Err(ApiError::Internal(error)),
        Err(error) => Err(ApiError::BadGateway(error.to_string())),
    }
}
