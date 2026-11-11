//! `/api/routes`: routes that an account saved to run again.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa_axum::router::{OpenApiRouter, UtoipaMethodRouterExt};
use utoipa_axum::routes;
use uuid::Uuid;

use super::filter::text;
use super::{ApiError, require_header};
use crate::authentication::CurrentUser;
use crate::database;
use crate::models::{
    ErrorBody, NewRoute, NewRouteFromActivity, RouteChange, RouteDetail, RouteImportQuery,
    RouteList, RoutePoint, RouteSummary,
};
use crate::routes::{prepare, route_points};
use crate::state::AppState;
use crate::track::{self, Point, Sample};

/// The largest GPX file that an import accepts.
const MAX_GPX_BYTES: usize = 32 * 1024 * 1024;
const MAX_NAME_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 5000;

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_routes, create_route))
        .routes(routes!(import_route).layer(DefaultBodyLimit::max(MAX_GPX_BYTES)))
        .routes(routes!(route, update_route, delete_route))
        .routes(routes!(route_file))
}

fn checked_name(name: Option<String>) -> Result<Option<String>, ApiError> {
    let name = text(name);
    if name
        .as_ref()
        .is_some_and(|name| name.chars().count() > MAX_NAME_CHARS)
    {
        return Err(ApiError::BadRequest("the name is too long"));
    }
    Ok(name)
}

fn checked_description(description: Option<String>) -> Result<Option<String>, ApiError> {
    let description = text(description);
    if description
        .as_ref()
        .is_some_and(|description| description.chars().count() > MAX_DESCRIPTION_CHARS)
    {
        return Err(ApiError::BadRequest("the description is too long"));
    }
    Ok(description)
}

async fn save(
    state: &AppState,
    user_id: i64,
    name: &str,
    description: Option<&str>,
    points: Vec<RoutePoint>,
    activity_id: Option<Uuid>,
) -> Result<RouteSummary, ApiError> {
    let prepared = prepare(points)?;
    let route = NewRoute {
        user_id,
        name,
        description,
        distance_m: prepared.distance_m,
        elevation_gain_m: prepared.elevation_gain_m,
        points: &serde_json::to_value(&prepared.points).map_err(anyhow::Error::from)?,
        preview: &serde_json::to_value(&prepared.preview).map_err(anyhow::Error::from)?,
        activity_id,
    };
    Ok(database::insert_route(&state.pool, &route).await?)
}

/// The account's saved routes, newest first.
#[utoipa::path(
    get,
    path = "/api/routes",
    responses(
        (status = 200, body = RouteList),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn list_routes(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<RouteList>, ApiError> {
    Ok(Json(RouteList {
        routes: database::list_routes(&state.pool, user.id).await?,
    }))
}

/// Saves the track of an activity as a route.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/routes",
    request_body = NewRouteFromActivity,
    responses(
        (status = 201, body = RouteSummary),
        (status = 400, description = "The activity has no track, or the name is too long", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such activity", body = ErrorBody),
    )
)]
pub(super) async fn create_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Json(request): Json<NewRouteFromActivity>,
) -> Result<(StatusCode, Json<RouteSummary>), ApiError> {
    require_header(&headers)?;
    let name = checked_name(request.name)?;
    let description = checked_description(request.description)?;
    let (activity, _) = database::activity_with_summary(&state.pool, user.id, request.activity_id)
        .await?
        .ok_or(ApiError::NotFound("no such activity"))?;
    let zip = state
        .files
        .original(activity.id)
        .await?
        .ok_or(ApiError::BadRequest("this activity has no track"))?;
    let id = activity.id;
    let samples = tokio::task::spawn_blocking(move || track::parse_samples(&zip))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|error| {
            tracing::warn!(%id, "could not read the activity file: {error:#}");
            ApiError::BadRequest("this activity has no track")
        })?;
    let points = route_points(&samples)?;
    let name = name.unwrap_or(activity.name);
    let route = save(
        &state,
        user.id,
        &name,
        description.as_deref(),
        points,
        Some(id),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(route)))
}

/// Saves the track or the route points of a GPX file as a route. The request
/// body is the file itself.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/routes/import",
    params(RouteImportQuery),
    request_body(content = Vec<u8>, content_type = "application/gpx+xml"),
    responses(
        (status = 201, body = RouteSummary),
        (status = 400, description = "Not a GPX file with at least two points", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub(super) async fn import_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<RouteImportQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<RouteSummary>), ApiError> {
    require_header(&headers)?;
    let name = checked_name(query.name)?.unwrap_or_else(|| "Imported route".to_owned());
    let samples = tokio::task::spawn_blocking(move || track::parse_gpx_route(&body))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|_| ApiError::BadRequest("this is not a GPX file Firn can read"))?;
    let points = route_points(&samples)?;
    let route = save(&state, user.id, &name, None, points, None).await?;
    Ok((StatusCode::CREATED, Json(route)))
}

/// One route with its points.
#[utoipa::path(
    get,
    path = "/api/routes/{id}",
    params(("id" = Uuid, Path, description = "The route's ID")),
    responses(
        (status = 200, body = RouteDetail),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub(super) async fn route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<RouteDetail>, ApiError> {
    let (route, points) = database::route(&state.pool, user.id, id)
        .await?
        .ok_or(ApiError::NotFound("no such route"))?;
    let points = serde_json::from_value(points).map_err(anyhow::Error::from)?;
    Ok(Json(RouteDetail { route, points }))
}

/// Changes the name and the description of a route.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    put,
    path = "/api/routes/{id}",
    params(("id" = Uuid, Path, description = "The route's ID")),
    request_body = RouteChange,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "No name, or a text that is too long", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub(super) async fn update_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(change): Json<RouteChange>,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let name = checked_name(Some(change.name))?.ok_or(ApiError::BadRequest("the name is empty"))?;
    let description = checked_description(change.description)?;
    if database::update_route(&state.pool, user.id, id, &name, description.as_deref()).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound("no such route"))
    }
}

/// Deletes a route. The activity it was made from stays.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/routes/{id}",
    params(("id" = Uuid, Path, description = "The route's ID")),
    responses(
        (status = 204, description = "Deleted, or there was none"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub(super) async fn delete_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    database::delete_route(&state.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Downloads a route as a GPX file.
#[utoipa::path(
    get,
    path = "/api/routes/{id}/file",
    params(("id" = Uuid, Path, description = "The route's ID")),
    responses(
        (status = 200, description = "The GPX file, as an attachment", content_type = "application/gpx+xml"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub(super) async fn route_file(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (route, points) = database::route(&state.pool, user.id, id)
        .await?
        .ok_or(ApiError::NotFound("no such route"))?;
    let points: Vec<RoutePoint> = serde_json::from_value(points).map_err(anyhow::Error::from)?;
    let samples: Vec<_> = points
        .into_iter()
        .map(|(lat, lon, elevation_m)| Sample {
            position: Some(Point { lat, lon }),
            elevation_m,
            ..Sample::default()
        })
        .collect();
    let gpx = track::to_gpx(&route.name, "route", &samples);
    Ok((
        [
            (header::CONTENT_TYPE, "application/gpx+xml".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"route-{id}.gpx\""),
            ),
        ],
        gpx,
    )
        .into_response())
}
