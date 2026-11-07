use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::auth::{self, Auth, CurrentUser};
use crate::db::{self, ActivityTrack, Filter, Pool};
use crate::garmin::GarminClient;
use crate::heatmap::{self, Position, Viewport};
use crate::statistics;
use crate::sync::{HealthRange, Report, sync_all};

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
    paths(
        heatmap,
        activities,
        activity_types,
        stats,
        sync_status,
        start_sync,
        auth::login,
        auth::callback,
        auth::logout,
        auth::me,
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
    /// How far back health data goes at least.
    pub health_history_days: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: Pool,
    pub(crate) garmin: Arc<GarminSettings>,
    pub(crate) auth: Arc<Auth>,
    /// By account.
    sync: Arc<Mutex<HashMap<i64, SyncStatus>>>,
    /// By account. Loaded on the first request and again after a sync, because the
    /// heatmap walks every track on every request, which would be slow from the database.
    tracks: Arc<tokio::sync::Mutex<HashMap<i64, Arc<Vec<ActivityTrack>>>>>,
}

impl AppState {
    pub fn new(pool: Pool, garmin: GarminSettings, auth: Auth) -> Self {
        Self {
            pool,
            garmin: Arc::new(garmin),
            auth: Arc::new(auth),
            sync: Arc::default(),
            tracks: Arc::default(),
        }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    async fn tracks(&self, user_id: i64) -> anyhow::Result<Arc<Vec<ActivityTrack>>> {
        let mut tracks = self.tracks.lock().await;
        if let Some(tracks) = tracks.get(&user_id) {
            return Ok(tracks.clone());
        }
        let loaded = Arc::new(db::load_tracks(&self.pool, user_id).await?);
        tracks.insert(user_id, loaded.clone());
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
        .route("/api/auth/login", get(auth::login))
        .route("/api/auth/callback", get(auth::callback))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route(
            "/api/me/garmin-key",
            put(auth::set_garmin_key).delete(auth::delete_garmin_key),
        )
        .route(
            "/api/statistics/activities",
            get(statistics::activity_statistics),
        )
        .route("/api/statistics/health", get(statistics::health_statistics))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .with_state(state)
}

#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    error: String,
}

pub enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    Forbidden(&'static str),
    Conflict(&'static str),
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
            ApiError::BadGateway(message) => (StatusCode::BAD_GATEWAY, message),
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

impl FilterQuery {
    fn into_filter(self, user_id: i64) -> Filter {
        let query = self;
        let text = |value: Option<String>| {
            value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Filter {
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
    let matching = db::matching_ids(&state.pool, &filter.into_filter(user.id)).await?;
    let tracks = state.tracks(user.id).await?;
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
    /// How many of the newest matching activities to skip, for the next page. Defaults to 0.
    offset: Option<i64>,
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
    let found =
        db::search_activities(&state.pool, &filter.into_filter(user.id), limit, offset).await?;
    let tracks = state.tracks(user.id).await?;
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

/// All activity types among the saved activities with GPS.
#[utoipa::path(
    get,
    path = "/api/activity-types",
    responses(
        (status = 200, body = ActivityTypeList),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
async fn activity_types(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<ActivityTypeList>, ApiError> {
    let types = db::activity_types(&state.pool, user.id).await?;
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
    let totals = db::totals(&state.pool, &filter.into_filter(user.id)).await?;
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
) -> Json<SyncStatus> {
    let statuses = state.sync.lock().unwrap();
    Json(statuses.get(&user.id).cloned().unwrap_or_default())
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
    let Some(sealed) = &user.garmin_api_key else {
        return Err(ApiError::Conflict(
            "No Garmin API key is saved. Add one in the settings",
        ));
    };
    let api_key = state.auth.secret_box.open(sealed)?;

    let status = SyncStatus::running(&Report::default());
    {
        let mut statuses = state.sync.lock().unwrap();
        let current = statuses.entry(user.id).or_default();
        if matches!(current.state, SyncState::Running) {
            return Err(ApiError::Conflict("a sync is already running"));
        }
        *current = status.clone();
    }

    tokio::spawn(run_sync(state, user.id, api_key));
    Ok((StatusCode::ACCEPTED, Json(status)))
}

async fn run_sync(state: AppState, user_id: i64, api_key: String) {
    let task = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut client = GarminClient::new(&state.garmin.base_url, &api_key)?;
            let set_status = |status: SyncStatus| {
                state.sync.lock().unwrap().insert(user_id, status);
            };
            sync_all(
                &state.pool,
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

    // Awaits a separate task, so that a panic cannot leave the state at running.
    let result = task.await;
    // Before the status changes, because that is what makes the page reload the map.
    state.tracks.lock().await.remove(&user_id);

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
            status.error = Some(error.to_string());
        }
        Err(error) => {
            tracing::error!(%error, "sync crashed");
            status.state = SyncState::Failed;
            status.error = Some("the sync crashed. See the logs".to_owned());
        }
    }
}
