//! `/api/auth`: signing in through the OpenID Connect provider, and out.

use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::{ApiError, require_header};
use crate::authentication::{self, LOGIN_COOKIE, SESSION_COOKIE, SESSION_DAYS, cookie, redirect};
use crate::database;
use crate::models::{CallbackQuery, ErrorBody};
use crate::secret;
use crate::state::AppState;

/// With a limit for each client, to slow down guessing.
pub(super) fn router() -> OpenApiRouter<AppState> {
    authentication::limited(
        OpenApiRouter::new()
            .routes(routes!(login))
            .routes(routes!(callback))
            .routes(routes!(logout)),
    )
}

/// Sends the browser to the OpenID Connect provider to sign in.
#[utoipa::path(
    get,
    path = "/api/auth/login",
    responses(
        (status = 303, description = "Redirects to the provider"),
        (status = 409, description = "OpenID Connect is not set up", body = ErrorBody),
        (status = 429, description = "Too many sign-ins are under way", body = ErrorBody),
    )
)]
pub(super) async fn login(State(state): State<AppState>) -> Result<Response, ApiError> {
    authentication::begin_login(&state.auth).await
}

/// Where the provider sends the browser back to. Starts a session and opens the app.
#[utoipa::path(
    get,
    path = "/api/auth/callback",
    params(CallbackQuery),
    responses((status = 303, description = "Opens the app, with `?login=failed` on failure"))
)]
pub(super) async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let clear_login = state.auth.cookie(LOGIN_COOKIE, "", 0, "/api/auth");
    match authentication::finish_login(&state, &headers, query).await {
        Ok(token) => {
            let session = state
                .auth
                .cookie(SESSION_COOKIE, &token, SESSION_DAYS * 24 * 3600, "/");
            redirect("/", vec![clear_login, session])
        }
        Err(error) => {
            tracing::warn!("sign-in failed: {error:#}");
            redirect("/?login=failed", vec![clear_login])
        }
    }
}

/// Ends the session.
#[utoipa::path(
    post,
    path = "/api/auth/logout",
    responses(
        (status = 204, description = "Signed out"),
        (status = 403, description = "The X-Requested-With header is missing", body = ErrorBody),
    )
)]
pub(super) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_header(&headers)?;
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        database::delete_session(&state.pool, &secret::sha256(token)).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, state.auth.cookie(SESSION_COOKIE, "", 0, "/"));
    Ok(response)
}
