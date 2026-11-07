use std::f64::consts::PI;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::db::{self, Bounds, GridCell, Pool};

const MAX_ZOOM: u8 = 22;
/// World width in pixels at zoom 0 for MapLibre's 512 px tiles.
const WORLD_PIXELS: f64 = 512.0;
const CELL_PIXELS: f64 = 2.0;
/// A cell is about 3 m here. GPS tracks are sampled every few meters, so finer
/// cells would break tracks into dots.
const MAX_GRID_ZOOM: u8 = 15;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "runmap",
        description = "Heatmap of runs saved from Garmin Connect"
    ),
    paths(heatmap, stats)
)]
pub struct ApiDoc;

pub fn router(pool: Pool) -> Router {
    Router::new()
        .route("/api/heatmap", get(heatmap))
        .route("/api/stats", get(stats))
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .with_state(pool)
}

#[derive(Serialize, ToSchema)]
struct ErrorBody {
    error: String,
}

enum ApiError {
    BadRequest(&'static str),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
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
    State(pool): State<Pool>,
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
    let cells = db::heatmap_cells(&pool, cell_size, &bounds)
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
        lon: x * 360.0 - 180.0,
        lat: (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees(),
        runs: cell.runs,
    }
}

#[derive(Serialize, ToSchema)]
struct Stats {
    runs: i64,
    distance_m: f64,
}

/// Totals across all saved runs.
#[utoipa::path(get, path = "/api/stats", responses((status = 200, body = Stats)))]
async fn stats(State(pool): State<Pool>) -> Result<Json<Stats>, ApiError> {
    let totals = db::totals(&pool).await?;
    Ok(Json(Stats {
        runs: totals.runs,
        distance_m: totals.distance_m,
    }))
}
