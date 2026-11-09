use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::AbortHandle;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use crate::auth::{self, Auth, CurrentUser};
use crate::db::{self, ActivityTrack, Filter, Pool};
use crate::files::FileStore;
use crate::garmin::{GarminClient, GarminError};
use crate::heatmap::{self, Position, Viewport};
use crate::routes;
use crate::statistics;
use crate::sync::{self, HealthRange, Report, sync_all};
use crate::track::{self, Sample};
use crate::transfer;

const MAX_ZOOM: u8 = 22;
/// About 1 cm. Fewer digits make large responses much smaller.
const COORDINATE_SCALE: f64 = 1e7;
const DEFAULT_SEARCH_LIMIT: i64 = 20;
const MAX_SEARCH_LIMIT: i64 = 100;
/// Enough to scrub along any activity, and few enough to keep the answer small.
const MAX_SAMPLES: usize = 3000;
/// Accounts whose tracks stay in memory. More would be rare on one server.
const MAX_CACHED_ACCOUNTS: usize = 32;

struct CachedTracks {
    tracks: Arc<tokio::sync::OnceCell<Arc<Vec<ActivityTrack>>>>,
    used: Instant,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "firn",
        description = "Heatmap of activities saved from Garmin Connect"
    ),
    paths(
        heatmap,
        activities,
        health,
        activity,
        activity_file,
        resync_activity,
        save_result,
        delete_result,
        activity_image,
        export,
        import,
        activity_types,
        training_blocks,
        routes::list_routes,
        routes::create_route,
        routes::import_route,
        routes::route,
        routes::update_route,
        routes::delete_route,
        routes::route_file,
        stats,
        sync_status,
        start_sync,
        cancel_sync,
        auth::login,
        auth::callback,
        auth::logout,
        auth::me,
        auth::delete_me,
        auth::set_garmin_key,
        auth::delete_garmin_key,
        statistics::activity_statistics,
        statistics::health_statistics,
    )
)]
pub struct ApiDoc;

pub struct GarminSettings {
    pub base_url: String,
    /// The key from before accounts existed, which the first account takes over.
    pub api_key: Option<String>,
    pub download_pause: Duration,
    /// The wait before retrying a failed Garmin request.
    pub retry_delay: Duration,
    /// How far back health data goes at least.
    pub health_history_days: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: Pool,
    pub(crate) garmin: Arc<GarminSettings>,
    pub(crate) auth: Arc<Auth>,
    pub(crate) files: FileStore,
    /// By account.
    sync: Arc<Mutex<HashMap<i64, SyncStatus>>>,
    /// The running syncs by account, to cancel them.
    sync_tasks: Arc<Mutex<HashMap<i64, AbortHandle>>>,
    /// By account. Loaded on the first request and again after a sync, because the
    /// heatmap walks every track on every request, which would be slow from the database.
    tracks: Arc<Mutex<HashMap<i64, CachedTracks>>>,
}

impl AppState {
    pub fn new(pool: Pool, files: FileStore, garmin: GarminSettings, auth: Auth) -> Self {
        Self {
            pool,
            files,
            garmin: Arc::new(garmin),
            auth: Arc::new(auth),
            sync: Arc::default(),
            sync_tasks: Arc::default(),
            tracks: Arc::default(),
        }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Stops the account's running sync. False if none runs.
    pub(crate) fn cancel_sync(&self, user_id: i64) -> bool {
        match self.sync_tasks.lock().unwrap().get(&user_id) {
            Some(task) => {
                task.abort();
                true
            }
            None => false,
        }
    }

    /// Loads each account's tracks once. Loading for one account never waits for another.
    async fn tracks(&self, user_id: i64) -> anyhow::Result<Arc<Vec<ActivityTrack>>> {
        let cell = {
            let mut cache = self.tracks.lock().unwrap();
            if !cache.contains_key(&user_id) && cache.len() >= MAX_CACHED_ACCOUNTS {
                // The account that went longest without a request makes room.
                if let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.used)
                    .map(|(id, _)| *id)
                {
                    cache.remove(&oldest);
                }
            }
            let cached = cache.entry(user_id).or_insert_with(|| CachedTracks {
                tracks: Arc::default(),
                used: Instant::now(),
            });
            cached.used = Instant::now();
            cached.tracks.clone()
        };
        let tracks = cell
            .get_or_try_init(|| async { db::load_tracks(&self.pool, user_id).await.map(Arc::new) })
            .await?;
        Ok(tracks.clone())
    }

    /// Makes the next request load the account's tracks again, after a sync changed them.
    pub(crate) fn forget_tracks(&self, user_id: i64) {
        self.tracks.lock().unwrap().remove(&user_id);
    }
}

/// Whether the backend runs and reaches its database, for container health checks.
#[utoipa::path(
    get,
    path = "/api/health",
    responses(
        (status = 204, description = "Healthy"),
        (status = 503, description = "The database cannot be reached"),
    )
)]
async fn health(State(state): State<AppState>) -> StatusCode {
    match db::ping(&state.pool).await {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(error) => {
            tracing::warn!("health check failed: {error:#}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// All routes. With `docs`, also the API documentation at /api/docs.
pub fn router(state: AppState) -> Router {
    router_with(state, true)
}

pub fn router_with(state: AppState, docs: bool) -> Router {
    let router = Router::new()
        .route("/api/health", get(health))
        .route("/api/heatmap", get(heatmap))
        .route("/api/activities", get(activities))
        .route("/api/activities/{id}", get(activity))
        .route("/api/activities/{id}/file", get(activity_file))
        .route("/api/activities/{id}/sync", post(resync_activity))
        .route(
            "/api/activities/{id}/result",
            put(save_result).delete(delete_result),
        )
        .route(
            "/api/activities/{id}/images/{image_id}",
            get(activity_image),
        )
        .route("/api/activity-types", get(activity_types))
        .route("/api/training-blocks", get(training_blocks))
        .route(
            "/api/routes",
            get(routes::list_routes).post(routes::create_route),
        )
        .route(
            "/api/routes/import",
            post(routes::import_route)
                .layer(axum::extract::DefaultBodyLimit::max(routes::MAX_GPX_BYTES)),
        )
        .route(
            "/api/routes/{id}",
            get(routes::route)
                .put(routes::update_route)
                .delete(routes::delete_route),
        )
        .route("/api/routes/{id}/file", get(routes::route_file))
        .route("/api/export", get(export))
        .route(
            "/api/import",
            post(import).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/stats", get(stats))
        .route(
            "/api/sync",
            get(sync_status).post(start_sync).delete(cancel_sync),
        )
        .route("/api/auth/login", get(auth::login))
        .route("/api/auth/callback", get(auth::callback))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me).delete(auth::delete_me))
        .route(
            "/api/me/garmin-key",
            put(auth::set_garmin_key).delete(auth::delete_garmin_key),
        )
        .route(
            "/api/statistics/activities",
            get(statistics::activity_statistics),
        )
        .route("/api/statistics/health", get(statistics::health_statistics))
        .with_state(state)
        // Logs each request with its status and duration, at debug level.
        .layer(tower_http::trace::TraceLayer::new_for_http());
    if docs {
        router.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
    } else {
        router
    }
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    error: String,
}

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    NotFound(&'static str),
    Forbidden(&'static str),
    Conflict(&'static str),
    TooManyRequests(&'static str),
    /// Garmin failed in a way the person can act on.
    BadGateway(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "sign in first".to_owned()),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message.to_owned()),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message.to_owned()),
            ApiError::BadGateway(message) => (StatusCode::BAD_GATEWAY, message),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message.to_owned()),
            ApiError::TooManyRequests(message) => {
                (StatusCode::TOO_MANY_REQUESTS, message.to_owned())
            }
            ApiError::Internal(error) => {
                tracing::error!("{error:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_owned(),
                )
            }
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        ApiError::Internal(error)
    }
}

/// Guards requests that change something. Browsers do not let other websites
/// send the `X-Requested-With` header without permission, which this API never grants.
pub fn require_header(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key("x-requested-with") {
        Ok(())
    } else {
        Err(ApiError::Forbidden("X-Requested-With header is required"))
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct HeatmapQuery {
    /// Map zoom level from 0 to 22. Sets how much detail the lines keep.
    zoom: u8,
    /// Western edge of the viewport, from -180 to 180 degrees.
    west: f64,
    /// Southern edge of the viewport, from -90 to 90 degrees.
    south: f64,
    /// Eastern edge of the viewport, from -180 to 180 degrees.
    east: f64,
    /// Northern edge of the viewport, from -90 to 90 degrees.
    north: f64,
}

/// Limits which activities count. Empty parameters are ignored and all others must match.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FilterQuery {
    /// One activity.
    id: Option<Uuid>,
    /// Words to find in the title, type, description, location or local start time.
    /// A length like `10km` or `45min` also finds activities within 5% of it.
    q: Option<String>,
    /// Comma-separated Garmin activity types, like `running,cycling`.
    types: Option<String>,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    min_distance_m: Option<f64>,
    max_distance_m: Option<f64>,
    min_duration_s: Option<f64>,
    max_duration_s: Option<f64>,
    /// First day to include, in local time.
    from: Option<NaiveDate>,
    /// Last day to include, in local time.
    to: Option<NaiveDate>,
    /// A training block, by the ID of the race that ends it. Leaves out the
    /// activities from before and after it. Unknown blocks are ignored.
    block: Option<Uuid>,
}

impl FilterQuery {
    pub async fn into_filter(self, pool: &Pool, user_id: i64) -> anyhow::Result<Filter> {
        let query = self;
        let block = match query.block {
            Some(id) => db::training_blocks_of(pool, user_id)
                .await?
                .into_iter()
                .find(|block| block.id == id),
            None => None,
        };
        // The block narrows the days that were asked for.
        let from = match (query.from, block.as_ref().and_then(|block| block.from)) {
            (Some(from), Some(start)) => Some(from.max(start)),
            (from, start) => from.or(start),
        };
        let to = match (query.to, block.as_ref().map(|block| block.to)) {
            (Some(to), Some(end)) => Some(to.min(end)),
            (to, end) => to.or(end),
        };
        Ok(Filter {
            user_id,
            id: query.id,
            query: text(query.q),
            activity_types: query
                .types
                .iter()
                .flat_map(|types| types.split(','))
                .map(str::trim)
                .filter(|activity_type| !activity_type.is_empty())
                .map(str::to_owned)
                .collect(),
            title: text(query.title),
            description: text(query.description),
            location: text(query.location),
            min_distance_m: query.min_distance_m,
            max_distance_m: query.max_distance_m,
            min_duration_s: query.min_duration_s,
            max_duration_s: query.max_duration_s,
            from,
            to,
            without_gps: false,
        })
    }
}

#[derive(Serialize, ToSchema)]
struct HeatLine {
    /// From 0 for the fewest activities in the viewport to 1 for the most, on a log scale.
    weight: f64,
    /// Longitude and latitude pairs.
    #[schema(value_type = Vec<Vec<f64>>)]
    path: Vec<Position>,
}

#[derive(Serialize, ToSchema)]
struct Heatmap {
    /// Sorted from the quietest line to the busiest.
    lines: Vec<HeatLine>,
}

/// Colored lines of the matching activities in a viewport.
#[utoipa::path(
    get,
    path = "/api/heatmap",
    params(HeatmapQuery, FilterQuery),
    responses(
        (status = 200, body = Heatmap),
        (status = 400, description = "Invalid viewport or zoom", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn heatmap(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<HeatmapQuery>,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<Heatmap>, ApiError> {
    if query.zoom > MAX_ZOOM {
        return Err(ApiError::BadRequest("zoom must be from 0 to 22"));
    }
    let longitudes = -180.0..=180.0;
    let latitudes = -90.0..=90.0;
    if !longitudes.contains(&query.west)
        || !longitudes.contains(&query.east)
        || !latitudes.contains(&query.south)
        || !latitudes.contains(&query.north)
        || query.west > query.east
        || query.south > query.north
    {
        return Err(ApiError::BadRequest("invalid viewport"));
    }

    let viewport = Viewport::from_degrees(query.west, query.south, query.east, query.north);
    let matching = db::matching_ids(
        &state.pool,
        &filter.into_filter(&state.pool, user.id).await?,
    )
    .await?;
    let tracks = state.tracks(user.id).await?;
    let lines = tokio::task::spawn_blocking(move || {
        let tracks: Vec<&[Position]> = tracks
            .iter()
            .filter(|activity| activity.bounds.intersects(&viewport))
            .filter(|activity| matching.contains(&activity.id))
            .map(|activity| activity.track.as_slice())
            .collect();
        heatmap::heat_lines(&tracks, &viewport, query.zoom)
    })
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(Heatmap {
        lines: lines
            .into_iter()
            .map(|line| HeatLine {
                weight: line.weight,
                path: line.path.into_iter().map(to_degrees).collect(),
            })
            .collect(),
    }))
}

/// West, south, east and north edges in degrees.
fn to_degree_bounds(bounds: Viewport) -> [f64; 4] {
    // Mercator y grows southward, so the largest y is the south edge.
    let [west, south] = to_degrees([bounds.min[0], bounds.max[1]]);
    let [east, north] = to_degrees([bounds.max[0], bounds.min[1]]);
    [west, south, east, north]
}

/// Trimmed text, or `None` if nothing is left.
fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn to_degrees(position: Position) -> Position {
    heatmap::unproject(position)
        .map(|degrees| (degrees * COORDINATE_SCALE).round() / COORDINATE_SCALE)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
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
struct ActivityItem {
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
    result: Option<db::RaceResult>,
}

impl ActivityItem {
    fn new(
        activity: db::ActivitySummary,
        bounds: Option<[f64; 4]>,
        result: Option<db::RaceResult>,
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
struct ActivityList {
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
async fn activities(
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
        None | Some("newest") => db::Order::Newest,
        Some("oldest") => db::Order::Oldest,
        Some("distance") => db::Order::Distance,
        Some("duration") => db::Order::Duration,
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
    let found = db::search_activities(&state.pool, &filter, order, limit, offset).await?;
    let tracks = state.tracks(user.id).await?;
    let wanted: HashSet<Uuid> = found.iter().map(|activity| activity.id).collect();
    let ids: Vec<Uuid> = wanted.iter().copied().collect();
    let mut results = db::race_results(&state.pool, &ids).await?;
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

/// What the device recorded along an activity, as columns of the same length.
/// A column is left out if the device did not record it, and a value is `null`
/// where it is missing.
#[derive(Serialize, ToSchema)]
struct Samples {
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
struct ActivityDetail {
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
    result: Option<db::RaceResult>,
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
async fn activity(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ActivityDetail>, ApiError> {
    let (activity, summary) = db::activity_with_summary(&state.pool, user.id, id)
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
    let images = db::image_ids(&state.pool, id)
        .await?
        .into_iter()
        .map(|image_id| format!("/api/activities/{id}/images/{image_id}"))
        .collect();
    let heart_rate_zones = db::document(&state.pool, user.id, "heart_rate_zones", "")
        .await?
        .and_then(|document| heart_rate_zones(&document, &activity.activity_type));
    let (previous_id, next_id) =
        db::neighbors(&state.pool, user.id, id, activity.started_at).await?;
    let result = db::race_results(&state.pool, &[id]).await?.remove(&id);
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
async fn resync_activity(
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

/// Enters the official result of a race, for the personal records. Replaces an
/// earlier one.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    put,
    path = "/api/activities/{id}/result",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    request_body = db::RaceResult,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "Invalid distance, time, target time or link", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such activity", body = ErrorBody),
    )
)]
async fn save_result(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(mut result): Json<db::RaceResult>,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let positive = |value: f64| value.is_finite() && value > 0.0;
    if !positive(result.distance_m) || !positive(result.time_s) {
        return Err(ApiError::BadRequest("distance and time must be positive"));
    }
    if result.target_time_s.is_some_and(|target| !positive(target)) {
        return Err(ApiError::BadRequest("the target time must be positive"));
    }
    result.event = text(result.event);
    result.results_url = text(result.results_url);
    // Only web links, so that the page never links to javascript: or the like.
    if let Some(url) = &result.results_url {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| ApiError::BadRequest("the results link is not a valid URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::BadRequest(
                "the results link must start with https://",
            ));
        }
    }
    if db::save_race_result(&state.pool, user.id, id, &result).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("no such activity"))
    }
}

/// Removes the official result of a race.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/activities/{id}/result",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    responses(
        (status = 204, description = "Removed, or there was none"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
async fn delete_result(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    db::delete_race_result(&state.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct FileQuery {
    /// `original` for the file the device recorded, usually FIT, or `gpx` for a
    /// GPX file made from it. Defaults to `original`.
    format: Option<String>,
}

/// Downloads an activity's file, as the device recorded it or as GPX.
#[utoipa::path(
    get,
    path = "/api/activities/{id}/file",
    params(("id" = Uuid, Path, description = "The activity's ID"), FileQuery),
    responses(
        (status = 200, description = "The file, as an attachment", content_type = "application/octet-stream"),
        (status = 400, description = "Unknown format", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such activity, or it has no file", body = ErrorBody),
    )
)]
async fn activity_file(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    let gpx = match query.format.as_deref() {
        None | Some("original") => false,
        Some("gpx") => true,
        Some(_) => return Err(ApiError::BadRequest("format must be original or gpx")),
    };
    let (activity, _) = db::activity_with_summary(&state.pool, user.id, id)
        .await?
        .ok_or(ApiError::NotFound("no such activity"))?;
    let zip = state
        .files
        .original(id)
        .await?
        .ok_or(ApiError::NotFound("this activity has no file"))?;
    let garmin_id = activity.garmin_id;
    let (file_name, content_type, data) = tokio::task::spawn_blocking(move || {
        if gpx {
            let samples = track::parse_samples(&zip)?;
            let gpx = track::to_gpx(&activity.name, &activity.activity_type, &samples);
            Ok::<_, anyhow::Error>((
                format!("{garmin_id}.gpx"),
                "application/gpx+xml",
                gpx.into_bytes(),
            ))
        } else {
            let (name, data) = track::original_file(&zip)?;
            let extension = name
                .rsplit_once('.')
                .map_or("fit", |(_, extension)| extension);
            Ok((
                format!("{garmin_id}.{extension}"),
                "application/octet-stream",
                data,
            ))
        }
    })
    .await
    .map_err(anyhow::Error::from)??;
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        data,
    )
        .into_response())
}

/// The largest ZIP that an import accepts.
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Downloads all of the account's data as a ZIP: activities with their files,
/// details and photos, and health data. Importing it restores everything.
#[utoipa::path(
    get,
    path = "/api/export",
    responses(
        (status = 200, description = "The ZIP, as an attachment", content_type = "application/zip"),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn export(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Response, ApiError> {
    let path = state.files.temporary_path("export").await?;
    let written = {
        let (pool, files, path) = (state.pool.clone(), state.files.clone(), path.clone());
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
            transfer::export(&handle, &pool, &files, user.id, file)
        })
        .await
        .map_err(anyhow::Error::from)
    };
    let opened = match written {
        Ok(Ok(())) => tokio::fs::File::open(&path)
            .await
            .map_err(anyhow::Error::from),
        Ok(Err(error)) | Err(error) => Err(error),
    };
    // The open file stays readable after it is removed, and nothing is left behind.
    let _ = tokio::fs::remove_file(&path).await;
    let file = opened?;
    let name = format!("firn-{}.zip", Utc::now().format("%Y-%m-%d"));
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ImportQuery {
    /// Leaves the account with what the import holds and nothing else: its
    /// other activities, routes and health data are removed. Does nothing if
    /// some files of the import could not be read. Defaults to `false`.
    replace: Option<bool>,
    /// Reads the ZIP as the export of the Apple Health app: `health` for only
    /// its daily values, like steps, sleep and heart rate, or `workouts` for
    /// those and its workouts, which become activities. Values that Garmin
    /// saved already stay.
    apple_health: Option<transfer::AppleHealthScope>,
}

/// Imports a ZIP: a Firn export, or FIT, GPX and TCX files from other apps and
/// devices, also in ZIPs of their own. Importing the same file again updates it.
/// Files from elsewhere get a negative ID, since they have none from Garmin.
///
/// Send the ZIP as the request body. Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/import",
    params(ImportQuery),
    request_body(content = Vec<u8>, content_type = "application/zip"),
    responses(
        (status = 200, body = transfer::ImportReport),
        (status = 400, description = "Not a ZIP, or too large", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "A sync is running", body = ErrorBody),
    )
)]
async fn import(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ImportQuery>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<transfer::ImportReport>, ApiError> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    require_header(&headers)?;
    // Both would write the same activities.
    if state.sync_tasks.lock().unwrap().contains_key(&user.id) {
        return Err(ApiError::Conflict("a sync is already running"));
    }
    let path = state.files.temporary_path("import").await?;
    let result = async {
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(anyhow::Error::from)?;
        let mut size = 0;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError::BadRequest("the upload broke off"))?;
            size += chunk.len() as u64;
            if size > MAX_IMPORT_BYTES {
                return Err(ApiError::BadRequest("the file is larger than 4 GB"));
            }
            file.write_all(&chunk).await.map_err(anyhow::Error::from)?;
        }
        file.flush().await.map_err(anyhow::Error::from)?;

        let (pool, files, path) = (state.pool.clone(), state.files.clone(), path.clone());
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            transfer::import(
                &handle,
                &pool,
                &files,
                user.id,
                &path,
                query.replace.unwrap_or(false),
                query.apple_health,
            )
        })
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|error| {
            tracing::warn!("import failed: {error:#}");
            ApiError::BadRequest("could not read this file. Is it a ZIP?")
        })
    }
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    state.forget_tracks(user.id);
    Ok(Json(result?))
}

/// A photo added to an activity on Garmin Connect.
#[utoipa::path(
    get,
    path = "/api/activities/{id}/images/{image_id}",
    params(
        ("id" = Uuid, Path, description = "The activity's ID"),
        ("image_id" = String, Path, description = "Garmin's image ID"),
    ),
    responses(
        (status = 200, description = "The image", content_type = "image/jpeg"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such image", body = ErrorBody),
    )
)]
async fn activity_image(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, image_id)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let content_type = db::image_type(&state.pool, user.id, id, &image_id)
        .await?
        .ok_or(ApiError::NotFound("no such image"))?;
    let data = state
        .files
        .image(id, &image_id)
        .await?
        .ok_or(ApiError::NotFound("no such image"))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            // An image never changes under its ID.
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        data,
    )
        .into_response())
}

#[derive(Serialize, ToSchema)]
struct ActivityType {
    /// Garmin's key for the type, like `trail_running`.
    activity_type: String,
    activities: i64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ActivityTypeQuery {
    /// Also counts activities without GPS, as the statistics do.
    #[serde(default)]
    without_gps: bool,
}

#[derive(Serialize, ToSchema)]
struct ActivityTypeList {
    /// Most frequent first.
    types: Vec<ActivityType>,
}

/// All activity types among the saved activities with GPS, or among all of them.
#[utoipa::path(
    get,
    path = "/api/activity-types",
    params(ActivityTypeQuery),
    responses(
        (status = 200, body = ActivityTypeList),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn activity_types(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ActivityTypeQuery>,
) -> Result<Json<ActivityTypeList>, ApiError> {
    let types = db::activity_types(&state.pool, user.id, query.without_gps).await?;
    Ok(Json(ActivityTypeList {
        types: types
            .into_iter()
            .map(|(activity_type, activities)| ActivityType {
                activity_type,
                activities,
            })
            .collect(),
    }))
}

#[derive(Serialize, ToSchema)]
struct Stats {
    activities: i64,
    distance_m: f64,
    /// West, south, east and north edges around all matching tracks, in degrees.
    #[schema(value_type = Option<Vec<f64>>)]
    bounds: Option<[f64; 4]>,
}

#[derive(Serialize, ToSchema)]
struct TrainingBlocks {
    /// Newest first.
    blocks: Vec<db::TrainingBlock>,
}

/// The training blocks of the races that have an official result. A block runs
/// from the last race that was at least as long to the race itself, so shorter
/// races in between do not end it. Pass its `id` as `block` to filter by it.
#[utoipa::path(
    get,
    path = "/api/training-blocks",
    responses(
        (status = 200, body = TrainingBlocks),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn training_blocks(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<TrainingBlocks>, ApiError> {
    Ok(Json(TrainingBlocks {
        blocks: db::training_blocks_of(&state.pool, user.id).await?,
    }))
}

/// Totals across the matching activities with GPS.
#[utoipa::path(
    get,
    path = "/api/stats",
    params(FilterQuery),
    responses(
        (status = 200, body = Stats),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn stats(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<Stats>, ApiError> {
    let filter = filter.into_filter(&state.pool, user.id).await?;
    let totals = db::totals(&state.pool, &filter).await?;
    let matching = db::matching_ids(&state.pool, &filter).await?;
    let tracks = state.tracks(user.id).await?;
    let bounds = tracks
        .iter()
        .filter(|activity| matching.contains(&activity.id))
        .map(|activity| activity.bounds)
        .reduce(|all, bounds| Viewport {
            min: [all.min[0].min(bounds.min[0]), all.min[1].min(bounds.min[1])],
            max: [all.max[0].max(bounds.max[0]), all.max[1].max(bounds.max[1])],
        });
    Ok(Json(Stats {
        activities: totals.activities,
        distance_m: totals.distance_m,
        bounds: bounds.map(to_degree_bounds),
    }))
}

#[derive(Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum SyncState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// A running sync lives in memory. The last finished one is also saved with the
/// account, so that it survives a restart. After a restart, the next sync finds
/// what is still missing.
#[derive(Clone, Default, Serialize, Deserialize, ToSchema)]
struct SyncStatus {
    state: SyncState,
    /// When the sync ended, for finished syncs.
    finished_at: Option<DateTime<Utc>>,
    /// Activities found on Garmin so far.
    listed: usize,
    /// Activities that this sync has to download.
    to_download: usize,
    downloaded: usize,
    /// Activities that could not be saved. The next sync retries them.
    failed: usize,
    /// Requests for health data, like sleep or HRV, that this sync has to make.
    health_to_fetch: usize,
    health_fetched: usize,
    /// Health data that could not be saved. The next sync retries it.
    health_failed: usize,
    error: Option<String>,
}

impl SyncStatus {
    fn running(report: &Report) -> Self {
        Self {
            state: SyncState::Running,
            listed: report.listed,
            to_download: report.to_download,
            downloaded: report.downloaded,
            failed: report.failed,
            health_to_fetch: report.health_to_fetch,
            health_fetched: report.health_fetched,
            health_failed: report.health_failed,
            error: None,
            finished_at: None,
        }
    }
}

/// Progress of the account's current or last sync.
#[utoipa::path(
    get,
    path = "/api/sync",
    responses(
        (status = 200, body = SyncStatus),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn sync_status(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<SyncStatus>, ApiError> {
    if let Some(status) = state.sync.lock().unwrap().get(&user.id) {
        return Ok(Json(status.clone()));
    }
    let saved = db::last_sync(&state.pool, user.id)
        .await?
        // A status saved by an older version may not fit, which is no reason to fail.
        .and_then(|saved| serde_json::from_value(saved).ok());
    Ok(Json(saved.unwrap_or_default()))
}

/// Stops the account's running sync. What it saved so far stays, and the next
/// sync continues from there.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/sync",
    responses(
        (status = 204, description = "The sync stopped"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "No sync is running", body = ErrorBody),
    )
)]
async fn cancel_sync(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    if state.cancel_sync(user.id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::Conflict("no sync is running"))
    }
}

/// Starts saving everything from Garmin that is not saved yet, in the background:
/// activities with their files and details, then health data like sleep and HRV.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/sync",
    responses(
        (status = 202, description = "The sync started", body = SyncStatus),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "A sync is running or no API key is set", body = ErrorBody),
    )
)]
async fn start_sync(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<SyncStatus>), ApiError> {
    require_header(&headers)?;
    let status = start_sync_of(&state, &user)?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}

/// Starts a sync of the account in the background, unless one is running.
fn start_sync_of(state: &AppState, user: &db::User) -> Result<SyncStatus, ApiError> {
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
        let users = match db::users_with_keys(&state.pool).await {
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
    if let Err(error) = db::save_last_sync(&state.pool, user_id, &saved).await {
        tracing::error!("{error:#}");
    }
}
