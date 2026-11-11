use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::heatmap::to_degree_bounds;
use super::{ApiError, AppState, ErrorBody, FilterQuery};
use crate::auth::CurrentUser;
use crate::database::{self, Filter};
use crate::heatmap::Viewport;
use crate::track::{self};

const DEFAULT_SEARCH_LIMIT: i64 = 20;
const MAX_SEARCH_LIMIT: i64 = 100;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct SearchQuery {
    /// How many activities to return, from 1 to 100. Defaults to 20.
    limit: Option<i64>,
    /// How many of the newest matching activities to skip, for the next page. Defaults to 0.
    offset: Option<i64>,
    /// Also finds activities without GPS. Defaults to false.
    #[serde(default)]
    without_gps: bool,
    /// `newest` (the default), `oldest`, `distance` or `duration`, the longest first.
    sort: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct ActivityItem {
    id: Uuid,
    /// Garmin's ID of the activity. Imported files have made-up, negative ones.
    garmin_id: i64,
    name: String,
    activity_type: String,
    started_at: DateTime<Utc>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    description: Option<String>,
    location: Option<String>,
    /// West, south, east and north edges of the track, in degrees.
    #[schema(value_type = Vec<f64>)]
    bounds: Option<[f64; 4]>,
    /// About 64 points along the track as `[latitude, longitude]`, to draw a
    /// small picture of the activity. `null` without a track.
    #[schema(value_type = Option<Vec<Vec<f64>>>)]
    preview: Option<Value>,
    /// Garmin's weather at the start, like `temp` in °F and `windSpeed`, if it listed one.
    #[schema(value_type = Option<Object>)]
    weather: Option<Value>,
    /// The watch and the sensors that recorded the activity, the watch first.
    /// `null` until the activity's file was searched for them.
    devices: Option<Vec<track::Device>>,
    /// What the activity trained most, like `TEMPO`, as Garmin names it.
    training_effect_label: Option<String>,
    /// The primary training effect, from 0 to 5.
    aerobic_training_effect: Option<f64>,
    /// The official result, if it was a race and one was entered.
    result: Option<database::RaceResult>,
}

impl ActivityItem {
    pub(super) fn new(
        activity: database::ActivitySummary,
        bounds: Option<[f64; 4]>,
        result: Option<database::RaceResult>,
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
pub(super) struct ActivityList {
    /// In the requested order, newest first by default.
    activities: Vec<ActivityItem>,
}

/// Searches saved activities.
#[utoipa::path(
    get,
    path = "/api/activities",
    params(SearchQuery, FilterQuery),
    responses(
        (status = 200, body = ActivityList),
        (status = 400, description = "Invalid limit or offset", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn activities(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(search): Query<SearchQuery>,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<ActivityList>, ApiError> {
    let limit = search.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest("limit must be from 1 to 100"));
    }
    let offset = search.offset.unwrap_or(0);
    if offset < 0 {
        return Err(ApiError::BadRequest("offset must not be negative"));
    }
    let order = match search.sort.as_deref() {
        None | Some("newest") => database::Order::Newest,
        Some("oldest") => database::Order::Oldest,
        Some("distance") => database::Order::Distance,
        Some("duration") => database::Order::Duration,
        Some(_) => {
            return Err(ApiError::BadRequest(
                "sort must be newest, oldest, distance or duration",
            ));
        }
    };
    let filter = Filter {
        without_gps: search.without_gps,
        ..filter.into_filter(&state.pool, user.id).await?
    };
    let found = database::search_activities(&state.pool, &filter, order, limit, offset).await?;
    let tracks = state.tracks(user.id).await?;
    let wanted: HashSet<Uuid> = found.iter().map(|activity| activity.id).collect();
    let ids: Vec<Uuid> = wanted.iter().copied().collect();
    let mut results = database::race_results(&state.pool, &ids).await?;
    let bounds: HashMap<Uuid, Viewport> = tracks
        .iter()
        .filter(|saved| wanted.contains(&saved.id))
        .map(|saved| (saved.id, saved.bounds))
        .collect();

    Ok(Json(ActivityList {
        activities: found
            .into_iter()
            .map(|activity| {
                let bounds = bounds.get(&activity.id).copied().map(to_degree_bounds);
                let result = results.remove(&activity.id);
                ActivityItem::new(activity, bounds, result)
            })
            .collect(),
    }))
}
