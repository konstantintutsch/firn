use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Query, State};
use uuid::Uuid;

use super::{ApiError, AppState, ErrorBody, FilterQuery};
use crate::auth::CurrentUser;
use crate::database;
use crate::heatmap::{Viewport, to_degree_bounds};
use crate::models;
use crate::models::{ActivityItem, ActivityList, Filter, SearchQuery};

const DEFAULT_SEARCH_LIMIT: i64 = 20;
const MAX_SEARCH_LIMIT: i64 = 100;

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
        None | Some("newest") => models::Order::Newest,
        Some("oldest") => models::Order::Oldest,
        Some("distance") => models::Order::Distance,
        Some("duration") => models::Order::Duration,
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
