use axum::Json;
use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::{ApiError, AppState, ErrorBody, require_header};
use crate::database;
use crate::garmin::{GarminClient, GarminError};

use super::{CurrentUser, SESSION_COOKIE};

/// Deletes the account with all its activities, files, health data and
/// sessions, and signs out. This cannot be undone.
///
/// Requires the `X-Requested-With` header.
#[utoipa::path(
    delete,
    path = "/api/me",
    responses(
        (status = 204, description = "The account is deleted"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The header is missing", body = ErrorBody),
    )
)]
pub async fn delete_me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_header(&headers)?;
    state.cancel_sync(user.id);
    // The files first: once the rows are gone, nothing says which files were the account's.
    for id in database::activity_ids(&state.pool, user.id).await? {
        state.files.remove_activity(id).await?;
    }
    database::delete_user(&state.pool, user.id).await?;
    state.forget_tracks(user.id);
    tracing::info!(user_id = user.id, "deleted an account");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, state.auth.cookie(SESSION_COOKIE, "", 0, "/"));
    Ok(response)
}

#[derive(Serialize, ToSchema)]
pub struct Me {
    id: i64,
    name: Option<String>,
    email: Option<String>,
    /// Whether a Garmin Personal API key is saved. The key itself is never returned.
    has_garmin_key: bool,
}

/// The signed-in account.
#[utoipa::path(
    get,
    path = "/api/me",
    responses(
        (status = 200, body = Me),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub async fn me(CurrentUser(user): CurrentUser) -> Json<Me> {
    Json(Me {
        id: user.id,
        name: user.name,
        email: user.email,
        has_garmin_key: user.garmin_api_key.is_some(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct GarminKey {
    /// Created at https://connect.garmin.com/app/settings/personalApiToken
    api_key: String,
}

/// Saves the account's Garmin Personal API key, after checking that Garmin accepts it.
#[utoipa::path(
    put,
    path = "/api/me/garmin-key",
    request_body = GarminKey,
    responses(
        (status = 204, description = "Saved"),
        (status = 400, description = "Garmin refused the key", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The X-Requested-With header is missing", body = ErrorBody),
    )
)]
pub async fn set_garmin_key(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
    Json(body): Json<GarminKey>,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    let key = body.api_key.trim();
    if key.is_empty() {
        return Err(ApiError::BadRequest("the API key is empty"));
    }
    let mut client = GarminClient::new(&state.garmin.base_url, key)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    match client.verify_key().await {
        Ok(()) => {}
        Err(GarminError::InvalidApiKey) => {
            return Err(ApiError::BadRequest("Garmin refused this API key"));
        }
        Err(error) => return Err(ApiError::BadGateway(error.to_string())),
    }
    database::set_garmin_key(
        &state.pool,
        user.id,
        Some(&state.auth.secret_box.seal(key)?),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Forgets the account's Garmin key. Saved data stays.
#[utoipa::path(
    delete,
    path = "/api/me/garmin-key",
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "The X-Requested-With header is missing", body = ErrorBody),
    )
)]
pub async fn delete_garmin_key(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_header(&headers)?;
    database::set_garmin_key(&state.pool, user.id, None).await?;
    // Without the key, a running sync should not keep using it.
    state.cancel_sync(user.id);
    Ok(StatusCode::NO_CONTENT)
}
