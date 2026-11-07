//! Data for the statistics page. The page groups and charts it itself.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Query, State};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::api::{ApiError, AppState, ErrorBody};
use crate::auth::CurrentUser;
use crate::db;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RangeQuery {
    /// First day to include, in local time.
    from: Option<NaiveDate>,
    /// Last day to include, in local time.
    to: Option<NaiveDate>,
}

impl RangeQuery {
    fn check(&self) -> Result<(), ApiError> {
        match (self.from, self.to) {
            (Some(from), Some(to)) if from > to => Err(ApiError::BadRequest("from is after to")),
            _ => Ok(()),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct ActivityStatistic {
    id: i64,
    name: String,
    activity_type: String,
    /// The local day it started.
    day: Option<String>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    moving_duration_s: Option<f64>,
    elevation_gain_m: Option<f64>,
    average_heart_rate: Option<f64>,
    max_heart_rate: Option<f64>,
    calories: Option<f64>,
    average_speed_mps: Option<f64>,
    vo2_max: Option<f64>,
    aerobic_training_effect: Option<f64>,
    anaerobic_training_effect: Option<f64>,
    training_load: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityStatistics {
    /// All activities, with or without GPS, oldest first.
    activities: Vec<ActivityStatistic>,
}

fn number(summary: &Value, key: &str) -> Option<f64> {
    summary.get(key)?.as_f64().filter(|value| value.is_finite())
}

/// Every activity in a range, with the values that the statistics chart.
#[utoipa::path(
    get,
    path = "/api/statistics/activities",
    params(RangeQuery),
    responses(
        (status = 200, body = ActivityStatistics),
        (status = 400, description = "Invalid range", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub async fn activity_statistics(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(range): Query<RangeQuery>,
) -> Result<Json<ActivityStatistics>, ApiError> {
    range.check()?;
    let records = db::activity_records(&state.pool, user.id, range.from, range.to).await?;
    Ok(Json(ActivityStatistics {
        activities: records
            .into_iter()
            .map(|record| {
                let summary = &record.summary;
                ActivityStatistic {
                    day: record
                        .started_local
                        .and_then(|local| local.get(..10).map(str::to_owned)),
                    moving_duration_s: number(summary, "movingDuration"),
                    elevation_gain_m: number(summary, "elevationGain"),
                    average_heart_rate: number(summary, "averageHR"),
                    max_heart_rate: number(summary, "maxHR"),
                    calories: number(summary, "calories"),
                    average_speed_mps: number(summary, "averageSpeed"),
                    vo2_max: number(summary, "vO2MaxValue"),
                    aerobic_training_effect: number(summary, "aerobicTrainingEffect"),
                    anaerobic_training_effect: number(summary, "anaerobicTrainingEffect"),
                    training_load: number(summary, "activityTrainingLoad"),
                    id: record.id,
                    name: record.name,
                    activity_type: record.activity_type,
                    distance_m: record.distance_m,
                    duration_s: record.duration_s,
                }
            })
            .collect(),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct HealthStatistics {
    /// Each metric, like `sleep_h` or `hrv`, with its days and values, oldest first.
    #[schema(value_type = Object, example = json!({"resting_heart_rate": [["2026-01-01", 48.0]]}))]
    metrics: BTreeMap<String, Vec<(NaiveDate, f64)>>,
}

/// Daily health values in a range, like sleep, HRV, resting heart rate and stress.
#[utoipa::path(
    get,
    path = "/api/statistics/health",
    params(RangeQuery),
    responses(
        (status = 200, body = HealthStatistics),
        (status = 400, description = "Invalid range", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub async fn health_statistics(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(range): Query<RangeQuery>,
) -> Result<Json<HealthStatistics>, ApiError> {
    range.check()?;
    let rows = db::daily_metrics(&state.pool, user.id, range.from, range.to).await?;
    let mut metrics: BTreeMap<String, Vec<(NaiveDate, f64)>> = BTreeMap::new();
    for (metric, day, value) in rows {
        metrics.entry(metric).or_default().push((day, value));
    }
    Ok(Json(HealthStatistics { metrics }))
}
