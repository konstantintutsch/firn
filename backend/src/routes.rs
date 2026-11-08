//! Routes that an account saved to run again, made from an activity or a GPX file.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::api::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::db::{self, NewRoute, RouteSummary};
use crate::track::{self, Point, Sample};

/// The largest GPX file that an import accepts.
pub const MAX_GPX_BYTES: usize = 32 * 1024 * 1024;
/// Enough to follow a route on a map, and few enough to keep the answer small.
const MAX_POINTS: usize = 5000;
const MAX_NAME_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 5000;
/// Smaller changes of elevation count as noise, which would otherwise add up to a climb.
const ELEVATION_NOISE_M: f64 = 3.0;
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// A point of a route: latitude, longitude and elevation in meters, if known.
type RoutePoint = (f64, f64, Option<f64>);

#[derive(Serialize, ToSchema)]
pub struct RouteList {
    routes: Vec<RouteSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct RouteDetail {
    #[serde(flatten)]
    route: RouteSummary,
    /// `[latitude, longitude, elevation]` in degrees and meters, with `null` for
    /// an unknown elevation.
    #[schema(value_type = Vec<Vec<Option<f64>>>)]
    points: Vec<RoutePoint>,
}

#[derive(Deserialize, ToSchema)]
pub struct NewRouteFromActivity {
    /// Garmin's ID of the activity to take the track from.
    activity_id: i64,
    /// Defaults to the activity's title.
    name: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct RouteChange {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// Defaults to `Imported route`.
    name: Option<String>,
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

/// Meters between two points along the Earth's surface.
fn haversine(a: &RoutePoint, b: &RoutePoint) -> f64 {
    let (lat_a, lat_b) = (a.0.to_radians(), b.0.to_radians());
    let across = ((b.1 - a.1).to_radians() / 2.0).sin();
    let along = ((lat_b - lat_a) / 2.0).sin();
    let h = along * along + lat_a.cos() * lat_b.cos() * across * across;
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

fn distance(points: &[RoutePoint]) -> f64 {
    points
        .windows(2)
        .map(|pair| haversine(&pair[0], &pair[1]))
        .sum()
}

/// The meters climbed, ignoring changes smaller than the noise of a barometer or GPS.
/// `None` if no point has an elevation.
fn elevation_gain(points: &[RoutePoint]) -> Option<f64> {
    let mut elevations = points.iter().filter_map(|point| point.2);
    let mut reference = elevations.next()?;
    let mut gain = 0.0;
    for elevation in elevations {
        if (elevation - reference).abs() >= ELEVATION_NOISE_M {
            gain += (elevation - reference).max(0.0);
            reference = elevation;
        }
    }
    Some(gain)
}

/// At most `MAX_POINTS` points, evenly spread, always with the first and the last.
fn thin(points: Vec<RoutePoint>) -> Vec<RoutePoint> {
    if points.len() <= MAX_POINTS {
        return points;
    }
    let last = points.len() - 1;
    (0..MAX_POINTS)
        .map(|index| points[index * last / (MAX_POINTS - 1)])
        .collect()
}

/// The samples with a position, as route points.
fn route_points(samples: &[Sample]) -> Result<Vec<RoutePoint>, ApiError> {
    let points: Vec<_> = samples
        .iter()
        .filter_map(|sample| {
            let Point { lat, lon } = sample.position?;
            Some((
                lat,
                lon,
                sample.elevation_m.filter(|value| value.is_finite()),
            ))
        })
        .collect();
    if points.len() < 2 {
        return Err(ApiError::BadRequest("the track has fewer than two points"));
    }
    Ok(thin(points))
}

async fn save(
    state: &AppState,
    user_id: i64,
    name: &str,
    description: Option<&str>,
    points: Vec<RoutePoint>,
    activity_id: Option<i64>,
) -> Result<RouteSummary, ApiError> {
    let route = NewRoute {
        user_id,
        name,
        description,
        distance_m: distance(&points),
        elevation_gain_m: elevation_gain(&points),
        points: &serde_json::to_value(&points).map_err(anyhow::Error::from)?,
        activity_id,
    };
    Ok(db::insert_route(&state.pool, &route).await?)
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
pub async fn list_routes(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<RouteList>, ApiError> {
    Ok(Json(RouteList {
        routes: db::list_routes(&state.pool, user.id).await?,
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
pub async fn create_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Json(request): Json<NewRouteFromActivity>,
) -> Result<(StatusCode, Json<RouteSummary>), ApiError> {
    require_header(&headers)?;
    let name = checked_name(request.name)?;
    let description = checked_description(request.description)?;
    let (activity, _) = db::activity_with_summary(&state.pool, user.id, request.activity_id)
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
            tracing::warn!(id, "could not read the activity file: {error:#}");
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
    params(ImportQuery),
    request_body(content = Vec<u8>, content_type = "application/gpx+xml"),
    responses(
        (status = 201, body = RouteSummary),
        (status = 400, description = "Not a GPX file with at least two points", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub async fn import_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ImportQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<RouteSummary>), ApiError> {
    require_header(&headers)?;
    let name = checked_name(query.name)?.unwrap_or_else(|| "Imported route".to_owned());
    let samples = tokio::task::spawn_blocking(move || track::parse_gpx_route(&body))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|_| ApiError::BadRequest("this is not a GPX file Runmap can read"))?;
    let points = route_points(&samples)?;
    let route = save(&state, user.id, &name, None, points, None).await?;
    Ok((StatusCode::CREATED, Json(route)))
}

/// One route with its points.
#[utoipa::path(
    get,
    path = "/api/routes/{id}",
    params(("id" = i64, Path, description = "The route's ID")),
    responses(
        (status = 200, body = RouteDetail),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub async fn route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> Result<Json<RouteDetail>, ApiError> {
    let (route, points) = db::route(&state.pool, user.id, id)
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
    params(("id" = i64, Path, description = "The route's ID")),
    request_body = RouteChange,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "No name, or a text that is too long", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub async fn update_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(change): Json<RouteChange>,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let name = checked_name(Some(change.name))?.ok_or(ApiError::BadRequest("the name is empty"))?;
    let description = checked_description(change.description)?;
    if db::update_route(&state.pool, user.id, id, &name, description.as_deref()).await? {
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
    params(("id" = i64, Path, description = "The route's ID")),
    responses(
        (status = 204, description = "Deleted, or there was none"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub async fn delete_route(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    db::delete_route(&state.pool, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Downloads a route as a GPX file.
#[utoipa::path(
    get,
    path = "/api/routes/{id}/file",
    params(("id" = i64, Path, description = "The route's ID")),
    responses(
        (status = 200, description = "The GPX file, as an attachment", content_type = "application/gpx+xml"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such route", body = ErrorBody),
    )
)]
pub async fn route_file(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let (route, points) = db::route(&state.pool, user.id, id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_degree_of_latitude() {
        let meters = distance(&[(0.0, 0.0, None), (1.0, 0.0, None)]);
        assert!((meters - 111_195.0).abs() < 10.0, "{meters}");
    }

    #[test]
    fn climbs_ignore_noise() {
        let points: Vec<RoutePoint> = [100.0, 101.0, 99.5, 110.0, 108.0, 120.0]
            .into_iter()
            .map(|elevation| (0.0, 0.0, Some(elevation)))
            .collect();
        // 100 to 110 and 110 to 120 are climbs. The dip to 108 is noise.
        assert_eq!(elevation_gain(&points), Some(20.0));
        assert_eq!(elevation_gain(&[(0.0, 0.0, None)]), None);
    }

    #[test]
    fn thinning_keeps_the_ends() {
        let points: Vec<RoutePoint> = (0..20_000)
            .map(|index| (f64::from(index), 0.0, None))
            .collect();
        let thinned = thin(points);
        assert_eq!(thinned.len(), MAX_POINTS);
        assert_eq!(thinned[0].0, 0.0);
        assert_eq!(thinned[MAX_POINTS - 1].0, 19_999.0);
    }
}
