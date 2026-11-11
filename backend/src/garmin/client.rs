use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde_json::Value;

use super::error::{is_temporary, read_limited, retry_after};
use super::{Activity, GarminError};

pub const DEFAULT_BASE_URL: &str = "https://personal.connectapi.garmin.com";
const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
/// Replaces a token this long before it expires, as Garmin's own client does.
const EXPIRY_MARGIN_SECONDS: i64 = 3600;
const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest original file that a sync saves. Even long activities stay far below.
const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;
/// The largest photo that a sync saves.
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
/// Image types that browsers show and that cannot carry scripts, unlike SVG.
const IMAGE_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/heic",
    "image/avif",
];

/// The media type to save and serve an image with. Anything but a known image
/// type becomes JPEG, so that a file never reaches a browser as a page or script.
pub fn image_type(content_type: Option<&str>) -> &'static str {
    let named = content_type
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .unwrap_or_default();
    IMAGE_TYPES
        .iter()
        .find(|known| known.eq_ignore_ascii_case(named))
        .copied()
        .unwrap_or("image/jpeg")
}

fn jwt_claim(token: &str, claim: &str) -> Option<Value> {
    let payload = URL_SAFE_NO_PAD.decode(token.split('.').nth(1)?).ok()?;
    serde_json::from_slice::<Value>(&payload)
        .ok()?
        .get_mut(claim)
        .map(Value::take)
}

fn expires_soon(token: &str, now: DateTime<Utc>) -> bool {
    jwt_claim(token, "exp")
        .and_then(|exp| exp.as_i64())
        .is_some_and(|exp| now.timestamp() > exp - EXPIRY_MARGIN_SECONDS)
}

pub struct GarminClient {
    http: Client,
    base_url: String,
    api_key: String,
    /// Valid for 24 hours and never stored: a new one only costs one request.
    access_token: Option<String>,
    /// The wait before the first retry of a failed request. Each further retry waits twice as long.
    retry_delay: Duration,
}

/// Tries per request. Server errors and lost connections are often gone a moment later.
const ATTEMPTS: u32 = 3;
/// The longest `Retry-After` that a request waits for. Longer limits stop the sync instead.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

impl GarminClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, GarminError> {
        let http = Client::builder()
            .user_agent(concat!("firn/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.trim().to_owned(),
            access_token: None,
            retry_delay: Duration::from_secs(2),
        })
    }

    /// Changes the wait before retries, for example to none in tests.
    pub fn with_retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    /// One page of activities of all types, newest first.
    pub async fn list_activities(
        &mut self,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Activity>, GarminError> {
        let url = format!(
            "{}/activitylist-service/activities/search/activities",
            self.base_url
        );
        let query = [("start", start.to_string()), ("limit", limit.to_string())];
        let response = self
            .send(|http| {
                http.get(&url)
                    .query(&query)
                    .header(ACCEPT, "application/json")
            })
            .await?;
        let summaries: Vec<Value> = response.json().await?;
        summaries.into_iter().map(Activity::from_summary).collect()
    }

    /// The activity's original file, in the ZIP that Garmin wraps it in.
    pub async fn download_original(&mut self, activity_id: i64) -> Result<Vec<u8>, GarminError> {
        let url = format!(
            "{}/download-service/files/activity/{activity_id}",
            self.base_url
        );
        let response = self
            .send(|http| {
                http.get(&url).header(
                    ACCEPT,
                    "application/zip, application/x-zip-compressed, application/octet-stream",
                )
            })
            .await?;
        read_limited(response, MAX_FILE_BYTES).await
    }

    /// An image behind one of Garmin's expiring links, with its media type. Sends
    /// no token: the link itself grants access, and it leads to another host.
    pub async fn download_image(&self, url: &str) -> Result<(Vec<u8>, String), GarminError> {
        let response = self.http.get(url).send().await?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::FORBIDDEN => return Err(GarminError::Forbidden),
            StatusCode::NOT_FOUND => return Err(GarminError::NotFound),
            status => return Err(GarminError::Status(status)),
        }
        let content_type = image_type(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
        )
        .to_owned();
        Ok((read_limited(response, MAX_IMAGE_BYTES).await?, content_type))
    }

    /// Details of one activity, like laps, heart rate and training effect.
    pub async fn activity_details(
        &mut self,
        activity_id: i64,
    ) -> Result<Option<Value>, GarminError> {
        self.get_json(&format!("/activity-service/activity/{activity_id}"), &[])
            .await
    }

    /// The workouts that Garmin associates with an activity, like a planned session.
    pub async fn activity_workouts(
        &mut self,
        activity_id: i64,
    ) -> Result<Option<Value>, GarminError> {
        self.get_json(
            &format!("/activity-service/activity/{activity_id}/workouts"),
            &[],
        )
        .await
    }

    /// Any JSON resource of the API. `None` if Garmin has nothing there.
    pub async fn get_json(
        &mut self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<Value>, GarminError> {
        let url = format!("{}{path}", self.base_url);
        let response = self
            .send(|http| {
                http.get(&url)
                    .query(query)
                    .header(ACCEPT, "application/json")
            })
            .await;
        let body = match response {
            Ok(response) => response.text().await?,
            Err(GarminError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        if body.trim().is_empty() {
            return Ok(None);
        }
        let value: Value = serde_json::from_str(&body)
            .map_err(|error| GarminError::Unexpected(format!("{path}: {error}")))?;
        Ok((!value.is_null()).then_some(value))
    }

    /// Checks that Garmin accepts the API key.
    pub async fn verify_key(&mut self) -> Result<(), GarminError> {
        self.fetch_token().await.map(|_| ())
    }

    /// Sends a request, and retries it after server errors, lost connections and
    /// short rate limits.
    async fn send(
        &mut self,
        build: impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Response, GarminError> {
        let mut attempt = 1;
        loop {
            let wait = match self.send_once(&build).await {
                Err(GarminError::RetryAfter(wait)) if attempt < ATTEMPTS => wait,
                Err(GarminError::RetryAfter(_)) => return Err(GarminError::RateLimited),
                Err(error @ (GarminError::Status(_) | GarminError::Http(_)))
                    if attempt < ATTEMPTS && is_temporary(&error) =>
                {
                    self.retry_delay * 2u32.pow(attempt - 1)
                }
                result => return result,
            };
            tracing::debug!(attempt, ?wait, "retrying a Garmin request");
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    async fn send_once(
        &mut self,
        build: &impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Response, GarminError> {
        let token = match &self.access_token {
            Some(token) if !expires_soon(token, Utc::now()) => token.clone(),
            _ => self.fetch_token().await?,
        };
        let mut response = build(&self.http).bearer_auth(token).send().await?;
        // A token can stop working before it expires, for example if it was revoked.
        if response.status() == StatusCode::UNAUTHORIZED {
            let token = self.fetch_token().await?;
            response = build(&self.http).bearer_auth(token).send().await?;
        }
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED => Err(GarminError::InvalidApiKey),
            StatusCode::FORBIDDEN => Err(GarminError::Forbidden),
            StatusCode::NOT_FOUND => Err(GarminError::NotFound),
            StatusCode::TOO_MANY_REQUESTS => Err(match retry_after(&response) {
                Some(wait) if wait <= MAX_RETRY_AFTER => GarminError::RetryAfter(wait),
                _ => GarminError::RateLimited,
            }),
            status => Err(GarminError::Status(status)),
        }
    }

    async fn fetch_token(&mut self) -> Result<String, GarminError> {
        let mut authorization = HeaderValue::from_str(&format!("ApiKey {}", self.api_key))
            .map_err(|_| GarminError::InvalidApiKey)?;
        authorization.set_sensitive(true);
        let response = self
            .http
            .post(format!("{}{TOKEN_PATH}", self.base_url))
            .header(AUTHORIZATION, authorization)
            .header(ACCEPT, "*/*")
            .send()
            .await?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(GarminError::InvalidApiKey);
            }
            StatusCode::TOO_MANY_REQUESTS => return Err(GarminError::RateLimited),
            status => return Err(GarminError::Status(status)),
        }
        let token = response.text().await?.trim().to_owned();
        if token.is_empty() {
            return Err(GarminError::Unexpected("empty access token".into()));
        }
        self.access_token = Some(token.clone());
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_image_types_stay() {
        assert_eq!(image_type(Some("image/png")), "image/png");
        assert_eq!(image_type(Some("IMAGE/WebP; q=1")), "image/webp");
        assert_eq!(image_type(Some("text/html")), "image/jpeg");
        assert_eq!(image_type(Some("image/svg+xml")), "image/jpeg");
        assert_eq!(image_type(None), "image/jpeg");
    }
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload.to_string()))
    }

    #[test]
    fn expiry_respects_margin() {
        let now = DateTime::from_timestamp(10_000, 0).unwrap();
        assert!(expires_soon(&jwt(json!({"exp": 10_000})), now));
        assert!(expires_soon(&jwt(json!({"exp": 13_599})), now));
        assert!(!expires_soon(&jwt(json!({"exp": 13_601})), now));
    }

    #[test]
    fn opaque_token_is_not_treated_as_expiring() {
        assert!(!expires_soon("opaque", Utc::now()));
        assert!(!expires_soon(&jwt(json!({"exp": "soon"})), Utc::now()));
    }
}
