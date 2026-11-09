use std::time::Duration;

use reqwest::{Response, StatusCode};

#[derive(Debug, thiserror::Error)]
pub enum GarminError {
    #[error(
        "Garmin refused the API key, which may have expired. \
         Create a new one at https://connect.garmin.com/app/settings/personalApiToken"
    )]
    InvalidApiKey,
    #[error("Garmin rate limited this client. Wait before trying again")]
    RateLimited,
    /// A short rate limit, which `send` waits out. Never leaves the client.
    #[error("Garmin asked to wait {0:?}")]
    RetryAfter(Duration),
    #[error("Garmin refused access to this resource")]
    Forbidden,
    #[error("Garmin has no such resource")]
    NotFound,
    #[error("Garmin answered with HTTP {0}")]
    Status(StatusCode),
    #[error("unexpected Garmin response: {0}")]
    Unexpected(String),
    #[error("could not reach Garmin: {0}")]
    Http(reqwest::Error),
    #[error(transparent)]
    Database(#[from] anyhow::Error),
}

/// Whether the same request may work a moment later.
pub(super) fn is_temporary(error: &GarminError) -> bool {
    match error {
        GarminError::Status(status) => status.is_server_error(),
        GarminError::Http(error) => error.is_timeout() || error.is_connect() || error.is_request(),
        _ => false,
    }
}

/// The wait that a rate limited response asks for, in seconds. Garmin may also
/// send a date there, which counts as unknown.
pub(super) fn retry_after(response: &Response) -> Option<Duration> {
    let seconds = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

/// Reads a response body, but stops at `limit` bytes instead of filling the memory.
pub(super) async fn read_limited(
    mut response: Response,
    limit: usize,
) -> Result<Vec<u8>, GarminError> {
    let too_large = || GarminError::Unexpected(format!("response larger than {limit} bytes"));
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl From<reqwest::Error> for GarminError {
    fn from(error: reqwest::Error) -> Self {
        GarminError::Http(error.without_url())
    }
}
