use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;

use super::{ApiError, AppState, ErrorBody, require_header};
use crate::auth::CurrentUser;
use crate::transfer;

/// The largest ZIP that an import accepts.
const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Downloads all of the account's data as a ZIP: activities with their files,
/// details and photos, and health data. Importing it restores everything.
#[utoipa::path(
    get,
    path = "/api/export",
    responses(
        (status = 200, description = "The ZIP, as an attachment", content_type = "application/zip"),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn export(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Response, ApiError> {
    let path = state.files.temporary_path("export").await?;
    let written = {
        let (pool, files, path) = (state.pool.clone(), state.files.clone(), path.clone());
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
            transfer::export(&handle, &pool, &files, user.id, file)
        })
        .await
        .map_err(anyhow::Error::from)
    };
    let opened = match written {
        Ok(Ok(())) => tokio::fs::File::open(&path)
            .await
            .map_err(anyhow::Error::from),
        Ok(Err(error)) | Err(error) => Err(error),
    };
    // The open file stays readable after it is removed, and nothing is left behind.
    let _ = tokio::fs::remove_file(&path).await;
    let file = opened?;
    let name = format!("firn-{}.zip", Utc::now().format("%Y-%m-%d"));
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ImportQuery {
    /// Leaves the account with what the import holds and nothing else: its
    /// other activities, routes and health data are removed. Does nothing if
    /// some files of the import could not be read. Defaults to `false`.
    replace: Option<bool>,
    /// Reads the ZIP as the export of the Apple Health app: `health` for only
    /// its daily values, like steps, sleep and heart rate, or `workouts` for
    /// those and its workouts, which become activities. Values that Garmin
    /// saved already stay.
    apple_health: Option<transfer::AppleHealthScope>,
}

/// Imports a ZIP: a Firn export, or FIT, GPX and TCX files from other apps and
/// devices, also in ZIPs of their own. Importing the same file again updates it.
/// Files from elsewhere get a negative ID, since they have none from Garmin.
///
/// Send the ZIP as the request body. Requires the `X-Requested-With` header.
#[utoipa::path(
    post,
    path = "/api/import",
    params(ImportQuery),
    request_body(content = Vec<u8>, content_type = "application/zip"),
    responses(
        (status = 200, body = transfer::ImportReport),
        (status = 400, description = "Not a ZIP, or too large", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
        (status = 409, description = "A sync is running", body = ErrorBody),
    )
)]
pub(super) async fn import(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(query): Query<ImportQuery>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Json<transfer::ImportReport>, ApiError> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    require_header(&headers)?;
    // Both would write the same activities.
    if state.sync_tasks.lock().unwrap().contains_key(&user.id) {
        return Err(ApiError::Conflict("a sync is already running"));
    }
    let path = state.files.temporary_path("import").await?;
    let result = async {
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(anyhow::Error::from)?;
        let mut size = 0;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ApiError::BadRequest("the upload broke off"))?;
            size += chunk.len() as u64;
            if size > MAX_IMPORT_BYTES {
                return Err(ApiError::BadRequest("the file is larger than 4 GB"));
            }
            file.write_all(&chunk).await.map_err(anyhow::Error::from)?;
        }
        file.flush().await.map_err(anyhow::Error::from)?;

        let (pool, files, path) = (state.pool.clone(), state.files.clone(), path.clone());
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            transfer::import(
                &handle,
                &pool,
                &files,
                user.id,
                &path,
                query.replace.unwrap_or(false),
                query.apple_health,
            )
        })
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|error| {
            tracing::warn!("import failed: {error:#}");
            ApiError::BadRequest("could not read this file. Is it a ZIP?")
        })
    }
    .await;
    let _ = tokio::fs::remove_file(&path).await;
    state.forget_tracks(user.id);
    Ok(Json(result?))
}
