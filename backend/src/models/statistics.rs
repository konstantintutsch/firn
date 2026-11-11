use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::RaceResult;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RangeQuery {
    /// First day to include, in local time.
    pub from: Option<NaiveDate>,
    /// Last day to include, in local time.
    pub to: Option<NaiveDate>,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityStatistic {
    pub id: Uuid,
    pub name: String,
    pub activity_type: String,
    /// The local day it started.
    pub day: Option<String>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub moving_duration_s: Option<f64>,
    pub elevation_gain_m: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
    pub calories: Option<f64>,
    pub average_speed_mps: Option<f64>,
    /// The VO2 max that the activity's file recorded.
    pub vo2_max: Option<f64>,
    pub aerobic_training_effect: Option<f64>,
    pub anaerobic_training_effect: Option<f64>,
    pub training_load: Option<f64>,
    /// The official result, for races that have one entered.
    pub result: Option<RaceResult>,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityStatistics {
    /// The matching activities, with or without GPS, oldest first.
    pub activities: Vec<ActivityStatistic>,
}

#[derive(Serialize, ToSchema)]
pub struct HealthStatistics {
    /// Each metric, like `sleep_h` or `hrv`, with its days and values, oldest first.
    /// `resting_heart_rate_7d` averages the resting heart rate over each day and the 6 before.
    #[schema(value_type = Object, example = json!({"resting_heart_rate": [["2026-01-01", 48.0]]}))]
    pub metrics: BTreeMap<String, Vec<(NaiveDate, f64)>>,
}
