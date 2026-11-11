use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::RaceResult;
use crate::database::schema::activities;
use crate::heatmap::COORDINATE_SCALE;
use crate::track::{self, Sample};

/// Enough to scrub along any activity, and few enough to keep the answer small.
const MAX_SAMPLES: usize = 3000;

#[derive(Insertable)]
#[diesel(table_name = activities)]
pub struct NewActivity<'a> {
    /// `None` gives the activity a new random ID, unless it is saved already.
    pub id: Option<Uuid>,
    pub garmin_id: i64,
    pub user_id: i64,
    pub name: &'a str,
    pub activity_type: &'a str,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub summary: &'a Value,
}

/// What a sync still has to download for an activity.
pub struct Missing {
    pub track: bool,
    pub details: bool,
    pub workouts: bool,
    pub images: bool,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = activities)]
pub struct ActivitySummary {
    pub id: Uuid,
    pub garmin_id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// About 64 points along the track as `[latitude, longitude]`.
    pub preview: Option<Value>,
    /// Garmin's weather at the start, if it listed one.
    pub weather: Option<Value>,
    /// The watch and sensors that recorded the activity.
    pub devices: Option<Value>,
    /// What the activity trained most, like `TEMPO`, as Garmin names it.
    pub training_effect_label: Option<String>,
    /// How much the activity improved aerobic fitness, from 0 to 5.
    pub aerobic_training_effect: Option<f64>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = activities)]
pub struct ActivityRecord {
    pub id: Uuid,
    pub name: String,
    pub activity_type: String,
    pub started_local: Option<String>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub summary: Value,
    pub vo2_max: Option<f64>,
}

/// Everything about activities that an export holds, apart from files.
pub struct ActivityExport {
    pub id: Uuid,
    pub summary: Value,
    pub details: Option<Value>,
    pub workouts: Option<Value>,
    /// Image IDs with their media types, in Garmin's order.
    pub images: Vec<(String, String)>,
}

/// Limits which activities count. All set fields must match.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// The account whose activities count.
    pub user_id: i64,
    pub id: Option<Uuid>,
    pub query: Option<String>,
    pub activity_types: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub min_distance_m: Option<f64>,
    pub max_distance_m: Option<f64>,
    pub min_duration_s: Option<f64>,
    pub max_duration_s: Option<f64>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Also counts activities without GPS, which have no place on a map but do in statistics.
    pub without_gps: bool,
}

/// How a search sorts the activities it finds.
#[derive(Debug, Clone, Copy)]
pub enum Order {
    Newest,
    Oldest,
    /// The longest distance first.
    Distance,
    /// The longest time first.
    Duration,
}

pub struct Totals {
    pub activities: i64,
    pub distance_m: f64,
}

/// Limits which activities count. Empty parameters are ignored and all others must match.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FilterQuery {
    /// One activity.
    pub id: Option<Uuid>,
    /// Words to find in the title, type, description, location or local start time.
    /// A length like `10km` or `45min` also finds activities within 5% of it.
    pub q: Option<String>,
    /// Comma-separated Garmin activity types, like `running,cycling`.
    pub types: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub min_distance_m: Option<f64>,
    pub max_distance_m: Option<f64>,
    pub min_duration_s: Option<f64>,
    pub max_duration_s: Option<f64>,
    /// First day to include, in local time.
    pub from: Option<NaiveDate>,
    /// Last day to include, in local time.
    pub to: Option<NaiveDate>,
    /// A training block, by the ID of the race that ends it. Leaves out the
    /// activities from before and after it. Unknown blocks are ignored.
    pub block: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// How many activities to return, from 1 to 100. Defaults to 20.
    pub limit: Option<i64>,
    /// How many of the newest matching activities to skip, for the next page. Defaults to 0.
    pub offset: Option<i64>,
    /// Also finds activities without GPS. Defaults to false.
    #[serde(default)]
    pub without_gps: bool,
    /// `newest` (the default), `oldest`, `distance` or `duration`, the longest first.
    pub sort: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityItem {
    pub id: Uuid,
    /// Garmin's ID of the activity. Imported files have made-up, negative ones.
    pub garmin_id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// West, south, east and north edges of the track, in degrees.
    #[schema(value_type = Vec<f64>)]
    pub bounds: Option<[f64; 4]>,
    /// About 64 points along the track as `[latitude, longitude]`, to draw a
    /// small picture of the activity. `null` without a track.
    #[schema(value_type = Option<Vec<Vec<f64>>>)]
    pub preview: Option<Value>,
    /// Garmin's weather at the start, like `temp` in °F and `windSpeed`, if it listed one.
    #[schema(value_type = Option<Object>)]
    pub weather: Option<Value>,
    /// The watch and the sensors that recorded the activity, the watch first.
    /// `null` until the activity's file was searched for them.
    pub devices: Option<Vec<track::Device>>,
    /// What the activity trained most, like `TEMPO`, as Garmin names it.
    pub training_effect_label: Option<String>,
    /// The primary training effect, from 0 to 5.
    pub aerobic_training_effect: Option<f64>,
    /// The official result, if it was a race and one was entered.
    pub result: Option<RaceResult>,
}

impl ActivityItem {
    pub fn new(
        activity: ActivitySummary,
        bounds: Option<[f64; 4]>,
        result: Option<RaceResult>,
    ) -> Self {
        ActivityItem {
            bounds,
            result,
            weather: activity.weather.filter(Value::is_object),
            devices: activity
                .devices
                .and_then(|devices| serde_json::from_value(devices).ok()),
            training_effect_label: activity.training_effect_label,
            aerobic_training_effect: activity.aerobic_training_effect,
            id: activity.id,
            garmin_id: activity.garmin_id,
            preview: activity.preview,
            name: activity.name,
            activity_type: activity.activity_type,
            started_at: activity.started_at,
            distance_m: activity.distance_m,
            duration_s: activity.duration_s,
            description: activity.description,
            location: activity.location,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct ActivityList {
    /// In the requested order, newest first by default.
    pub activities: Vec<ActivityItem>,
}

/// What the device recorded along an activity, as columns of the same length.
/// A column is left out if the device did not record it, and a value is `null`
/// where it is missing.
#[derive(Serialize, ToSchema)]
pub struct Samples {
    /// Seconds since the first sample, including pauses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<Vec<Option<f64>>>,
    /// Longitude and latitude pairs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<Option<Vec<f64>>>>)]
    pub position: Option<Vec<Option<[f64; 2]>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation_m: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heart_rate: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_m_s: Option<Vec<Option<f64>>>,
    /// Revolutions, or strides of one foot, per minute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cadence: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_w: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<Vec<Option<f64>>>,
}

impl Samples {
    /// Keeps every nth sample, so that at most `MAX_SAMPLES` remain.
    pub fn from_samples(samples: &[Sample]) -> Self {
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
pub struct ActivityDetail {
    pub activity: ActivityItem,
    /// The activity exactly as Garmin listed it, with all its metrics.
    #[schema(value_type = Object)]
    pub summary: serde_json::Value,
    pub samples: Samples,
    /// The URLs of the photos added to the activity on Garmin Connect, in Garmin's order.
    pub images: Vec<String>,
    /// The lowest heart rate of zones 1 to 5 and the maximum, from the zones
    /// that Garmin has for this sport, or else its default ones.
    pub heart_rate_zones: Option<Vec<f64>>,
    /// `fit`, `gpx` or `tcx`: the kind of file the device recorded, or `None` without one.
    pub original_format: Option<String>,
    /// The official result, if it was a race and one was entered.
    pub result: Option<RaceResult>,
    /// The account's activity that started just before this one.
    pub previous_id: Option<Uuid>,
    /// The account's activity that started just after this one.
    pub next_id: Option<Uuid>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FileQuery {
    /// `original` for the file the device recorded, usually FIT, or `gpx` for a
    /// GPX file made from it. Defaults to `original`.
    pub format: Option<String>,
}
