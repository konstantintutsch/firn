use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{ApiError, AppState, ErrorBody, FilterQuery};
use crate::auth::CurrentUser;
use crate::db::{self};
use crate::heatmap::{self, Position, Viewport};

const MAX_ZOOM: u8 = 22;
/// About 1 cm. Fewer digits make large responses much smaller.
pub(super) const COORDINATE_SCALE: f64 = 1e7;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct HeatmapQuery {
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

#[derive(Serialize, ToSchema)]
pub(super) struct HeatLine {
    /// From 0 for the fewest activities in the viewport to 1 for the most, on a log scale.
    weight: f64,
    /// Longitude and latitude pairs.
    #[schema(value_type = Vec<Vec<f64>>)]
    path: Vec<Position>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct Heatmap {
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
pub(super) async fn heatmap(
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
pub(super) fn to_degree_bounds(bounds: Viewport) -> [f64; 4] {
    // Mercator y grows southward, so the largest y is the south edge.
    let [west, south] = to_degrees([bounds.min[0], bounds.max[1]]);
    let [east, north] = to_degrees([bounds.max[0], bounds.min[1]]);
    [west, south, east, north]
}

fn to_degrees(position: Position) -> Position {
    heatmap::unproject(position)
        .map(|degrees| (degrees * COORDINATE_SCALE).round() / COORDINATE_SCALE)
}
