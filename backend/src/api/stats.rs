use axum::Json;
use axum::extract::{Query, State};

use super::{ApiError, AppState, ErrorBody, FilterQuery};
use crate::auth::CurrentUser;
use crate::database;
use crate::heatmap::{Viewport, to_degree_bounds};
use crate::models::Stats;

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
pub(super) async fn stats(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(filter): Query<FilterQuery>,
) -> Result<Json<Stats>, ApiError> {
    let filter = filter.into_filter(&state.pool, user.id).await?;
    let totals = database::totals(&state.pool, &filter).await?;
    let matching = database::matching_ids(&state.pool, &filter).await?;
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
