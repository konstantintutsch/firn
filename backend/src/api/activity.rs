use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use super::activities::ActivityItem;
use super::heatmap::COORDINATE_SCALE;
use super::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::database;
use crate::garmin::{GarminClient, GarminError};
use crate::sync::{self};
use crate::track::{self, Sample};

/// Enough to scrub along any activity, and few enough to keep the answer small.
const MAX_SAMPLES: usize = 3000;

/// What the device recorded along an activity, as columns of the same length.
/// A column is left out if the device did not record it, and a value is `null`
/// where it is missing.
#[derive(Serialize, ToSchema)]
pub(super) struct Samples {
    /// Seconds since the first sample, including pauses.
    #[serde(skip_serializing_if = "Option::is_none")]
    time_s: Option<Vec<Option<f64>>>,
    /// Longitude and latitude pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<Option<Vec<f64>>>>)]
    position: Option<Vec<Option<[f64; 2]>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    distance_m: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elevation_m: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    heart_rate: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed_m_s: Option<Vec<Option<f64>>>,
    /// Revolutions, or strides of one foot, per minute.
    #[serde(skip_serializing_if = "Option::is_none")]
    cadence: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    power_w: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature_c: Option<Vec<Option<f64>>>,
}

impl Samples {
    /// Keeps every nth sample, so that at most `MAX_SAMPLES` remain.
    fn from_samples(samples: &[Sample]) -> Self {
        let step = samples.len().div_ceil(MAX_SAMPLES).max(1);
        let kept: Vec<&Sample> = samples.iter().step_by(step).collect();
        let start = samples.iter().find_map(|sample| sample.timestamp);
        let column = |value: fn(&Sample) -> Option<f64>| {
            let values: Vec<Option<f64>> = kept
                .iter()
                .map(|sample| value(sample).map(|value| (value * 100.0).round() / 100.0))
                .collect();
            values.iter().any(Option::is_some).then_some(values)
        };
        let positions: Vec<Option<[f64; 2]>> = kept
            .iter()
            .map(|sample| {
                sample.position.map(|point| {
                    [point.lon, point.lat]
                        .map(|degrees| (degrees * COORDINATE_SCALE).round() / COORDINATE_SCALE)
                })
            })
            .collect();
        Samples {
            time_s: start.map(|start| {
                kept.iter()
                    .map(|sample| sample.timestamp.map(|time| time - start))
                    .collect()
            }),
            position: positions.iter().any(Option::is_some).then_some(positions),
            distance_m: column(|sample| sample.distance_m),
            elevation_m: column(|sample| sample.elevation_m),
            heart_rate: column(|sample| sample.heart_rate),
            speed_m_s: column(|sample| sample.speed_m_s),
            cadence: column(|sample| sample.cadence),
            power_w: column(|sample| sample.power_w),
            temperature_c: column(|sample| sample.temperature_c),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(super) struct ActivityDetail {
    activity: ActivityItem,
    /// The activity exactly as Garmin listed it, with all its metrics.
    #[schema(value_type = Object)]
    summary: serde_json::Value,
    samples: Samples,
    /// The URLs of the photos added to the activity on Garmin Connect, in Garmin's order.
    images: Vec<String>,
    /// The lowest heart rate of zones 1 to 5 and the maximum, from the zones
    /// that Garmin has for this sport, or else its default ones.
    heart_rate_zones: Option<Vec<f64>>,
    /// `fit`, `gpx` or `tcx`: the kind of file the device recorded, or `None` without one.
    original_format: Option<String>,
    /// The official result, if it was a race and one was entered.
    result: Option<database::RaceResult>,
    /// The account's activity that started just before this one.
    previous_id: Option<Uuid>,
    /// The account's activity that started just after this one.
    next_id: Option<Uuid>,
}

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
