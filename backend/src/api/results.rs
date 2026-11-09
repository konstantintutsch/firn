use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use super::query::text;
use super::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::db::{self};

/// Enters the official result of a race, for the personal records. Replaces an
/// earlier one.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    put,
    path = "/api/activities/{id}/result",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    request_body = db::RaceResult,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "Invalid distance, time, target time or link", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such activity", body = ErrorBody),
    )
)]
pub(super) async fn save_result(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(mut result): Json<db::RaceResult>,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let positive = |value: f64| value.is_finite() && value > 0.0;
    if !positive(result.distance_m) || !positive(result.time_s) {
        return Err(ApiError::BadRequest("distance and time must be positive"));
    }
    if result.target_time_s.is_some_and(|target| !positive(target)) {
        return Err(ApiError::BadRequest("the target time must be positive"));
    }
    result.event = text(result.event);
    result.results_url = text(result.results_url);
    // Only web links, so that the page never links to javascript: or the like.
    if let Some(url) = &result.results_url {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| ApiError::BadRequest("the results link is not a valid URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::BadRequest(
                "the results link must start with https://",
            ));
        }
    }
    if db::save_race_result(&state.pool, user.id, id, &result).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("no such activity"))
    }
}

/// Removes the official result of a race.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/activities/{id}/result",
    params(("id" = Uuid, Path, description = "The activity's ID")),
    responses(
        (status = 204, description = "Removed, or there was none"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub(super) async fn delete_result(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    db::delete_race_result(&state.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
