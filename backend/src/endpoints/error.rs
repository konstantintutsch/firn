use crate::models::ErrorBody;
use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    NotFound(&'static str),
    Forbidden(&'static str),
    Conflict(&'static str),
    TooManyRequests(&'static str),
    /// Garmin failed in a way the person can act on.
    BadGateway(String),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_owned()),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "sign in first".to_owned()),
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message.to_owned()),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message.to_owned()),
            ApiError::BadGateway(message) => (StatusCode::BAD_GATEWAY, message),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message.to_owned()),
            ApiError::TooManyRequests(message) => {
                (StatusCode::TOO_MANY_REQUESTS, message.to_owned())
            }
            ApiError::Internal(error) => {
                tracing::error!("{error:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_owned(),
                )
            }
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        ApiError::Internal(error)
    }
}

/// Guards requests that change something. Browsers do not let other websites
/// send the `X-Requested-With` header without permission, which this API never grants.
pub fn require_header(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key("x-requested-with") {
        Ok(())
    } else {
        Err(ApiError::Forbidden("X-Requested-With header is required"))
    }
}
