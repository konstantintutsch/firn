use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{ApiError, AppState, ErrorBody};
use crate::auth::CurrentUser;
use crate::db::{self};

#[derive(Serialize, ToSchema)]
pub(super) struct ActivityType {
    /// Garmin's key for the type, like `trail_running`.
    activity_type: String,
    activities: i64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ActivityTypeQuery {
    /// Also counts activities without GPS, as the statistics do.
    #[serde(default)]
    without_gps: bool,
}

#[derive(Serialize, ToSchema)]
pub(super) struct ActivityTypeList {
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
pub(super) async fn activity_types(
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
