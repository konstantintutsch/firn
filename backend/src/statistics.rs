//! Data for the statistics page. The page groups and charts it itself.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Query, State};
use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::api::{ApiError, AppState, ErrorBody, FilterQuery};
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

fn check_range(from: Option<NaiveDate>, to: Option<NaiveDate>) -> Result<(), ApiError> {
    match (from, to) {
        (Some(from), Some(to)) if from > to => Err(ApiError::BadRequest("from is after to")),
        _ => Ok(()),
    }
}

/// Days in the rolling average of the resting heart rate.
const RESTING_AVERAGE_DAYS: u64 = 7;

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
    /// The official result, for races that have one entered.
    result: Option<db::RaceResult>,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityStatistics {
    /// The matching activities, with or without GPS, oldest first.
    activities: Vec<ActivityStatistic>,
}

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
pub async fn activity_statistics(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<ActivityStatistics>, ApiError> {
    let filter = db::Filter {
        without_gps: true,
        ..filter.into_filter(user.id)
    };
    check_range(filter.from, filter.to)?;
    let records = db::activity_records(&state.pool, &filter).await?;
    let ids: Vec<i64> = records.iter().map(|record| record.id).collect();
    let mut results = db::race_results(&state.pool, &ids).await?;
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

#[derive(Serialize, ToSchema)]
pub struct HealthStatistics {
    /// Each metric, like `sleep_h` or `hrv`, with its days and values, oldest first.
    /// `resting_heart_rate_7d` averages the resting heart rate over each day and the 6 before.
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
    check_range(range.from, range.to)?;
    // The days before the range only feed the rolling average of its first days.
    let lead_in = range
        .from
        .and_then(|from| from.checked_sub_days(Days::new(RESTING_AVERAGE_DAYS - 1)));
    let rows = db::daily_metrics(&state.pool, user.id, lead_in.or(range.from), range.to).await?;
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

/// For each day with a value, the average of the values on it and the `days - 1` days before.
fn rolling_average(values: &[(NaiveDate, f64)], days: u64) -> Vec<(NaiveDate, f64)> {
    let mut start = 0;
    let mut total = 0.0;
    values
        .iter()
        .enumerate()
        .map(|(end, &(day, value))| {
            total += value;
            while values[start].0 + Days::new(days) <= day {
                total -= values[start].1;
                start += 1;
            }
            (day, total / (end + 1 - start) as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_average_skips_days_out_of_the_window() {
        let day = |text: &str| text.parse::<NaiveDate>().unwrap();
        let values = [
            (day("2026-01-01"), 50.0),
            (day("2026-01-02"), 48.0),
            (day("2026-01-07"), 46.0),
            (day("2026-01-08"), 40.0),
        ];
        assert_eq!(
            rolling_average(&values, 7),
            [
                (day("2026-01-01"), 50.0),
                (day("2026-01-02"), 49.0),
                (day("2026-01-07"), 48.0),
                (day("2026-01-08"), 44.666666666666664),
            ]
        );
    }
}
