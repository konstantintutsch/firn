use axum::extract::FromRequestParts;
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::api::{ApiError, AppState};
use crate::database;
use crate::models::User;
use crate::secret::{self};

pub const SESSION_COOKIE: &str = "firn_session";
pub(super) const SESSION_DAYS: i64 = 30;

/// The signed-in account. Answers 401 without a valid session.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        database::session_user(&state.pool, &secret::sha256(token))
            .await?
            .map(CurrentUser)
            .ok_or(ApiError::Unauthorized)
    }
}

pub(super) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
}

pub(super) fn redirect(location: &str, cookies: Vec<HeaderValue>) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    headers.insert(
        LOCATION,
        HeaderValue::from_str(location).expect("locations are URL-safe"),
    );
    for cookie in cookies {
        headers.append(SET_COOKIE, cookie);
    }
    response
}
