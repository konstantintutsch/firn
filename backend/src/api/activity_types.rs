use axum::Json;
use axum::extract::{Query, State};

use super::{ApiError, ErrorBody};
use crate::authentication::CurrentUser;
use crate::database;
use crate::models::{ActivityType, ActivityTypeList, ActivityTypeQuery};
use crate::state::AppState;

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
    let types = database::activity_types(&state.pool, user.id, query.without_gps).await?;
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
