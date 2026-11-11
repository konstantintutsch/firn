use chrono::NaiveDate;
use diesel::prelude::*;
use uuid::Uuid;

use crate::schema::race_results;

/// The official result of a race, entered by hand.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Queryable,
    Selectable,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[diesel(table_name = race_results)]
pub struct RaceResult {
    /// The race's distance, like 21097.5 for a half marathon.
    pub distance_m: f64,
    /// The official finish time.
    pub time_s: f64,
    /// The name of the race, if it differs from the activity's.
    pub event: Option<String>,
    /// Where the official results are published.
    pub results_url: Option<String>,
    /// The finish time that was aimed for.
    #[serde(default)]
    pub target_time_s: Option<f64>,
}

/// One race, as the training blocks need it.
pub struct RaceDay {
    pub id: Uuid,
    pub name: String,
    pub day: NaiveDate,
    pub distance_m: f64,
}
