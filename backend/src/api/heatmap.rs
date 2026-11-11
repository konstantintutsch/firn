use axum::Json;
use axum::extract::{Query, State};

use super::{ApiError, AppState, ErrorBody, FilterQuery};
use crate::auth::CurrentUser;
use crate::database;
use crate::heatmap::{self, Position, Viewport, to_degrees};
use crate::models::{HeatLine, Heatmap, HeatmapQuery};

const MAX_ZOOM: u8 = 22;

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
    let matching = database::matching_ids(
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
