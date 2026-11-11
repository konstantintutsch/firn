use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{ApiError, AppState, ErrorBody};
use crate::auth::CurrentUser;
use crate::database;
use crate::garmin::image_type;
use crate::track::{self};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct FileQuery {
    /// `original` for the file the device recorded, usually FIT, or `gpx` for a
    /// GPX file made from it. Defaults to `original`.
    format: Option<String>,
}

/// Downloads an activity's file, as the device recorded it or as GPX.
#[utoipa::path(
    get,
    path = "/api/activities/{id}/file",
    params(("id" = Uuid, Path, description = "The activity's ID"), FileQuery),
    responses(
        (status = 200, description = "The file, as an attachment", content_type = "application/octet-stream"),
        (status = 400, description = "Unknown format", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such activity, or it has no file", body = ErrorBody),
    )
)]
pub(super) async fn activity_file(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<Uuid>,
    Query(query): Query<FileQuery>,
) -> Result<Response, ApiError> {
    let gpx = match query.format.as_deref() {
        None | Some("original") => false,
        Some("gpx") => true,
        Some(_) => return Err(ApiError::BadRequest("format must be original or gpx")),
    };
    let (activity, _) = database::activity_with_summary(&state.pool, user.id, id)
        .await?
        .ok_or(ApiError::NotFound("no such activity"))?;
    let zip = state
        .files
        .original(id)
        .await?
        .ok_or(ApiError::NotFound("this activity has no file"))?;
    let garmin_id = activity.garmin_id;
    let (file_name, content_type, data) = tokio::task::spawn_blocking(move || {
        if gpx {
            let samples = track::parse_samples(&zip)?;
            let gpx = track::to_gpx(&activity.name, &activity.activity_type, &samples);
            Ok::<_, anyhow::Error>((
                format!("{garmin_id}.gpx"),
                "application/gpx+xml",
                gpx.into_bytes(),
            ))
        } else {
            let (name, data) = track::original_file(&zip)?;
            let extension = name
                .rsplit_once('.')
                .map_or("fit", |(_, extension)| extension);
            Ok((
                format!("{garmin_id}.{extension}"),
                "application/octet-stream",
                data,
            ))
        }
    })
    .await
    .map_err(anyhow::Error::from)??;
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        data,
    )
        .into_response())
}

/// A photo added to an activity on Garmin Connect.
#[utoipa::path(
    get,
    path = "/api/activities/{id}/images/{image_id}",
    params(
        ("id" = Uuid, Path, description = "The activity's ID"),
        ("image_id" = String, Path, description = "Garmin's image ID"),
    ),
    responses(
        (status = 200, description = "The image", content_type = "image/jpeg"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "No such image", body = ErrorBody),
    )
)]
pub(super) async fn activity_image(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path((id, image_id)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let content_type = database::image_type(&state.pool, user.id, id, &image_id)
        .await?
        .ok_or(ApiError::NotFound("no such image"))?;
    let data = state
        .files
        .image(id, &image_id)
        .await?
        .ok_or(ApiError::NotFound("no such image"))?;
    Ok((
        [
            // Types saved by older versions were not checked.
            (
                header::CONTENT_TYPE,
                image_type(Some(&content_type)).to_owned(),
            ),
            // Should a browser still open it as a page, nothing in it runs.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox".to_owned(),
            ),
            // An image never changes under its ID.
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        data,
    )
        .into_response())
}
