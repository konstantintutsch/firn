//! `/api/statistics`: data for the statistics page. The page groups and charts it itself.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Query, State};
use chrono::{Days, NaiveDate};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::ApiError;
use crate::authentication::CurrentUser;
use crate::database;
use crate::models::{
    ActivityStatistic, ActivityStatistics, ErrorBody, Filter, FilterQuery, HealthStatistics,
    RangeQuery,
};
use crate::state::AppState;
use crate::statistics::rolling_average;

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(activity_statistics))
        .routes(routes!(health_statistics))
}

fn check_range(from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<(), ApiError> {
    match (from, to) {
        (Some(from), Some(to)) if from > to => Err(ApiError::BadRequest("from is after to")),
        _ => Ok(()),
    }
}

/// Days in the rolling average of the resting heart rate.
const RESTING_AVERAGE_DAYS: u64 = 7;

fn number(summary: &Value, key: &str) -> Option<f64> {
    summary.get(key)?.as_f64().filter(|value| value.is_finite())
}

/// The matching activities, with the values that the statistics chart.
#[utoipa::path(
    get,
    path = "/api/statistics/activities",
    params(FilterQuery),
    responses(
        (status = 200, body = ActivityStatistics),
        (status = 400, description = "Invalid range", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn activity_statistics(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<ActivityStatistics>, ApiError> {
    let filter = Filter {
        without_gps: true,
        ..filter.into_filter(&state.pool, user.id).await?
    };
    check_range(filter.from, filter.to)?;
    let records = database::activity_records(&state.pool, &filter).await?;
    let ids: Vec<Uuid> = records.iter().map(|record| record.id).collect();
    let mut results = database::race_results(&state.pool, &ids).await?;
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
                    vo2_max: record.vo2_max,
                    aerobic_training_effect: number(summary, "aerobicTrainingEffect"),
                    anaerobic_training_effect: number(summary, "anaerobicTrainingEffect"),
                    training_load: number(summary, "activityTrainingLoad"),
                    result: results.remove(&record.id),
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
pub(super) async fn health_statistics(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(range): Query<RangeQuery>,
) -> Result<Json<HealthStatistics>, ApiError> {
    check_range(range.from, range.to)?;
    // The days before the range only feed the rolling average of its first days.
    let lead_in = range
        .from
        .and_then(|from| from.checked_sub_days(Days::new(RESTING_AVERAGE_DAYS - 1)));
    let rows =
        database::daily_metrics(&state.pool, user.id, lead_in.or(range.from), range.to).await?;
    let mut metrics: BTreeMap<String, Vec<(NaiveDate, f64)>> = BTreeMap::new();
    for (metric, day, value) in rows {
        metrics.entry(metric).or_default().push((day, value));
    }
    if let Some(resting) = metrics.get("resting_heart_rate") {
        let average = rolling_average(resting, RESTING_AVERAGE_DAYS);
        metrics.insert("resting_heart_rate_7d".to_owned(), average);
    }
    if let Some(from) = range.from {
        for values in metrics.values_mut() {
            values.retain(|(day, _)| *day >= from);
        }
        metrics.retain(|_, values| !values.is_empty());
    }
    Ok(Json(HealthStatistics { metrics }))
}
