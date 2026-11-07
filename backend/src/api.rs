use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::db::{self, ActivityTrack, Filter, Pool};
use crate::garmin::GarminClient;
use crate::heatmap::{self, Position, Viewport};
use crate::sync::{Report, sync_activities};

const MAX_ZOOM: u8 = 22;
/// About 1 cm. Fewer digits make large responses much smaller.
const COORDINATE_SCALE: f64 = 1e7;
const DEFAULT_SEARCH_LIMIT: i64 = 20;
const MAX_SEARCH_LIMIT: i64 = 100;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "runmap",
        description = "Heatmap of activities saved from Garmin Connect"
    ),
    paths(heatmap, activities, activity_types, stats, sync_status, start_sync)
)]
pub struct ApiDoc;

pub struct GarminSettings {
    pub base_url: String,
    pub api_key: Option<String>,
    pub download_pause: Duration,
}

#[derive(Clone)]
pub struct AppState {
    pool: Pool,
    garmin: Arc<GarminSettings>,
    sync: Arc<Mutex<SyncStatus>>,
    /// Loaded on the first request and again after a sync, because the heatmap
    /// walks every track on every request, which would be slow from the database.
    tracks: Arc<tokio::sync::Mutex<Option<Arc<Vec<ActivityTrack>>>>>,
}

impl AppState {
    pub fn new(pool: Pool, garmin: GarminSettings) -> Self {
        Self {
            pool,
            garmin: Arc::new(garmin),
            sync: Arc::default(),
            tracks: Arc::default(),
        }
    }

    async fn tracks(&self) -> anyhow::Result<Arc<Vec<ActivityTrack>>> {
        let mut tracks = self.tracks.lock().await;
        if let Some(tracks) = tracks.as_ref() {
            return Ok(tracks.clone());
        }
        let loaded = Arc::new(db::load_tracks(&self.pool).await?);
        *tracks = Some(loaded.clone());
        Ok(loaded)
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/heatmap", get(heatmap))
        .route("/api/activities", get(activities))
        .route("/api/activity-types", get(activity_types))
        .route("/api/stats", get(stats))
        .route("/api/sync", get(sync_status).post(start_sync))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .with_state(state)
}

#[derive(Serialize, ToSchema)]
struct ErrorBody {
    error: String,
}

enum ApiError {
    BadRequest(&'static str),
    Forbidden(&'static str),
    Conflict(&'static str),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message.to_owned()),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message.to_owned()),
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
struct FilterQuery {
    /// One activity.
    id: Option<i64>,
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
}

impl From<FilterQuery> for Filter {
    fn from(query: FilterQuery) -> Self {
        let text = |value: Option<String>| {
            value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Self {
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
            from: query.from,
            to: query.to,
        }
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
    )
)]
async fn heatmap(
    State(state): State<AppState>,
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
    let matching = db::matching_ids(&state.pool, &filter.into()).await?;
    let tracks = state.tracks().await?;
    let lines = tokio::task::spawn_blocking(move || {
        let tracks: Vec<&[Position]> = tracks
            .iter()
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

fn to_degrees(position: Position) -> Position {
    heatmap::unproject(position)
        .map(|degrees| (degrees * COORDINATE_SCALE).round() / COORDINATE_SCALE)
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    /// How many activities to return, from 1 to 100. Defaults to 20.
    limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
struct ActivityItem {
    id: i64,
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
}

#[derive(Serialize, ToSchema)]
struct ActivityList {
    /// Newest first.
    activities: Vec<ActivityItem>,
}

/// Searches saved activities.
#[utoipa::path(
    get,
    path = "/api/activities",
    params(SearchQuery, FilterQuery),
    responses(
        (status = 200, body = ActivityList),
        (status = 400, description = "Invalid limit", body = ErrorBody),
    )
)]
async fn activities(
    State(state): State<AppState>,
    Query(search): Query<SearchQuery>,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<ActivityList>, ApiError> {
    let limit = search.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(ApiError::BadRequest("limit must be from 1 to 100"));
    }
    let found = db::search_activities(&state.pool, &filter.into(), limit).await?;
    let tracks = state.tracks().await?;
    let bounds: HashMap<i64, Viewport> = tracks
        .iter()
        .filter(|saved| found.iter().any(|activity| activity.id == saved.id))
        .filter_map(|saved| Some((saved.id, heatmap::bounds(&saved.track)?)))
        .collect();

    Ok(Json(ActivityList {
        activities: found
            .into_iter()
            .map(|activity| ActivityItem {
                bounds: bounds.get(&activity.id).map(|bounds| {
                    // Mercator y grows southward, so the largest y is the south edge.
                    let [west, south] = to_degrees([bounds.min[0], bounds.max[1]]);
                    let [east, north] = to_degrees([bounds.max[0], bounds.min[1]]);
                    [west, south, east, north]
                }),
                id: activity.id,
                name: activity.name,
                activity_type: activity.activity_type,
                started_at: activity.started_at,
                distance_m: activity.distance_m,
                duration_s: activity.duration_s,
                description: activity.description,
                location: activity.location,
            })
            .collect(),
    }))
}

#[derive(Serialize, ToSchema)]
struct ActivityType {
    /// Garmin's key for the type, like `trail_running`.
    activity_type: String,
    activities: i64,
}

#[derive(Serialize, ToSchema)]
struct ActivityTypeList {
    /// Most frequent first.
    types: Vec<ActivityType>,
}

/// All activity types among the saved activities.
#[utoipa::path(
    get,
    path = "/api/activity-types",
    responses((status = 200, body = ActivityTypeList))
)]
async fn activity_types(State(state): State<AppState>) -> Result<Json<ActivityTypeList>, ApiError> {
    let types = db::activity_types(&state.pool).await?;
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
}

/// Totals across the matching activities.
#[utoipa::path(
    get,
    path = "/api/stats",
    params(FilterQuery),
    responses((status = 200, body = Stats))
)]
async fn stats(
    State(state): State<AppState>,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<Stats>, ApiError> {
    let totals = db::totals(&state.pool, &filter.into()).await?;
    Ok(Json(Stats {
        activities: totals.activities,
        distance_m: totals.distance_m,
    }))
}

#[derive(Clone, Copy, Default, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum SyncState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
}

/// Lives in memory: after a restart, the next sync finds what is still missing.
#[derive(Clone, Default, Serialize, ToSchema)]
struct SyncStatus {
    state: SyncState,
    /// Activities with GPS found on Garmin so far.
    listed: usize,
    /// Tracks that this sync has to download.
    to_download: usize,
    downloaded: usize,
    /// Tracks that could not be saved. The next sync retries them.
    failed: usize,
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
            error: None,
        }
    }
}

/// Progress of the current or last sync.
#[utoipa::path(get, path = "/api/sync", responses((status = 200, body = SyncStatus)))]
async fn sync_status(State(state): State<AppState>) -> Json<SyncStatus> {
    Json(state.sync.lock().unwrap().clone())
}

/// Starts saving all activities with GPS that are not saved yet, in the background.
///
/// Requires the `X-Requested-With` header. Browsers do not let other websites
/// send it without permission, which this API never grants.
#[utoipa::path(
    post,
    path = "/api/sync",
    responses(
        (status = 202, description = "The sync started", body = SyncStatus),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "A sync is running or no API key is set", body = ErrorBody),
    )
)]
async fn start_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<SyncStatus>), ApiError> {
    if !headers.contains_key("x-requested-with") {
        return Err(ApiError::Forbidden("X-Requested-With header is required"));
    }
    let Some(api_key) = state.garmin.api_key.clone() else {
        return Err(ApiError::Conflict(
            "GARMIN_PERSONAL_API_KEY is not set. Add it to .env and run just up",
        ));
    };

    let status = SyncStatus::running(&Report::default());
    {
        let mut current = state.sync.lock().unwrap();
        if matches!(current.state, SyncState::Running) {
            return Err(ApiError::Conflict("a sync is already running"));
        }
        *current = status.clone();
    }

    tokio::spawn(run_sync(state, api_key));
    Ok((StatusCode::ACCEPTED, Json(status)))
}

async fn run_sync(state: AppState, api_key: String) {
    let task = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut client = GarminClient::new(&state.garmin.base_url, &api_key)?;
            sync_activities(
                &state.pool,
                &mut client,
                state.garmin.download_pause,
                |report| *state.sync.lock().unwrap() = SyncStatus::running(report),
            )
            .await
        })
    };

    // Awaits a separate task, so that a panic cannot leave the state at running.
    let result = task.await;
    // Before the status changes, because that is what makes the page reload the map.
    *state.tracks.lock().await = None;

    let mut status = state.sync.lock().unwrap();
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
            status.error = Some(error.to_string());
        }
        Err(error) => {
            tracing::error!(%error, "sync crashed");
            status.state = SyncState::Failed;
            status.error = Some("the sync crashed. See the logs".to_owned());
        }
    }
}
