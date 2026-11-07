use std::f64::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::db::{self, Bounds, GridCell, Pool};
use crate::garmin::GarminClient;
use crate::sync::{Report, sync_runs};

const MAX_ZOOM: u8 = 22;
/// World width in pixels at zoom 0 for MapLibre's 512 px tiles.
const WORLD_PIXELS: f64 = 512.0;
const CELL_PIXELS: f64 = 1.0;
/// The map's maximum zoom, where a cell is about 20 cm: far below GPS accuracy.
const MAX_GRID_ZOOM: u8 = 18;
/// About 1 cm, well below a cell. Fewer digits make large responses much smaller.
const COORDINATE_SCALE: f64 = 1e7;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "runmap",
        description = "Heatmap of runs saved from Garmin Connect"
    ),
    paths(heatmap, stats, sync_status, start_sync)
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
}

impl AppState {
    pub fn new(pool: Pool, garmin: GarminSettings) -> Self {
        Self {
            pool,
            garmin: Arc::new(garmin),
            sync: Arc::default(),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/heatmap", get(heatmap))
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
    /// Map zoom level from 0 to 22. Sets the grid resolution, up to the finest grid.
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

#[derive(Serialize, ToSchema)]
struct HeatCell {
    lon: f64,
    lat: f64,
    /// Number of distinct runs that passed through this cell.
    runs: i64,
}

#[derive(Serialize, ToSchema)]
struct Heatmap {
    /// Zoom level that the grid was built for. Lower than the requested zoom
    /// when that exceeds the finest grid.
    grid_zoom: u8,
    cells: Vec<HeatCell>,
}

/// Grid cells with run counts for a viewport.
#[utoipa::path(
    get,
    path = "/api/heatmap",
    params(HeatmapQuery),
    responses(
        (status = 200, body = Heatmap),
        (status = 400, description = "Invalid viewport or zoom", body = ErrorBody),
    )
)]
async fn heatmap(
    State(state): State<AppState>,
    Query(query): Query<HeatmapQuery>,
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

    let grid_zoom = query.zoom.min(MAX_GRID_ZOOM);
    let cell_size = cell_size(grid_zoom);
    let bounds = Bounds {
        west: query.west,
        south: query.south,
        east: query.east,
        north: query.north,
    };
    let cells = db::heatmap_cells(&state.pool, cell_size, &bounds)
        .await?
        .iter()
        .map(|cell| to_heat_cell(cell, cell_size))
        .collect();
    Ok(Json(Heatmap { grid_zoom, cells }))
}

fn cell_size(zoom: u8) -> f64 {
    CELL_PIXELS / (WORLD_PIXELS * 2f64.powi(zoom.into()))
}

fn to_heat_cell(cell: &GridCell, cell_size: f64) -> HeatCell {
    let x = (cell.ix as f64 + 0.5) * cell_size;
    let y = (cell.iy as f64 + 0.5) * cell_size;
    HeatCell {
        lon: round_coordinate(x * 360.0 - 180.0),
        lat: round_coordinate((PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees()),
        runs: cell.runs,
    }
}

fn round_coordinate(degrees: f64) -> f64 {
    (degrees * COORDINATE_SCALE).round() / COORDINATE_SCALE
}

#[derive(Serialize, ToSchema)]
struct Stats {
    runs: i64,
    distance_m: f64,
}

/// Totals across all saved runs.
#[utoipa::path(get, path = "/api/stats", responses((status = 200, body = Stats)))]
async fn stats(State(state): State<AppState>) -> Result<Json<Stats>, ApiError> {
    let totals = db::totals(&state.pool).await?;
    Ok(Json(Stats {
        runs: totals.runs,
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
    /// Runs found on Garmin so far.
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

/// Starts saving all runs that are not saved yet, in the background.
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
            sync_runs(
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
