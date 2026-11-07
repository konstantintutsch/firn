//! Client for the Garmin Connect Personal API.
//!
//! See <https://developer.garmin.com/gc-developer-program/personal/swagger-docs/>.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://personal.connectapi.garmin.com";
const TOKEN_PATH: &str = "/personalapitoken-service/personal-api/personal-api-key/token";
/// Replaces a token this long before it expires, as Garmin's own client does.
const EXPIRY_MARGIN_SECONDS: i64 = 3600;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum GarminError {
    #[error(
        "Garmin refused the API key, which may have expired. \
         Create a new one at https://connect.garmin.com/app/settings/personalApiToken"
    )]
    InvalidApiKey,
    #[error("Garmin rate limited this client. Wait before trying again")]
    RateLimited,
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

impl From<reqwest::Error> for GarminError {
    fn from(error: reqwest::Error) -> Self {
        GarminError::Http(error.without_url())
    }
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

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    /// False for indoor activities. Virtual ones also count as false: their route
    /// is in a virtual world, not where the athlete was.
    pub has_track: bool,
    pub summary: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActivity {
    activity_id: i64,
    activity_name: Option<String>,
    #[serde(rename = "startTimeGMT")]
    start_time_gmt: String,
    activity_type: RawActivityType,
    distance: Option<f64>,
    duration: Option<f64>,
    has_polyline: Option<bool>,
    start_latitude: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActivityType {
    type_key: String,
}

impl Activity {
    fn from_summary(summary: Value) -> Result<Self, GarminError> {
        let raw = RawActivity::deserialize(&summary)
            .map_err(|error| GarminError::Unexpected(format!("activity: {error}")))?;
        let started_at = NaiveDateTime::parse_from_str(&raw.start_time_gmt, "%Y-%m-%d %H:%M:%S")
            .map_err(|error| GarminError::Unexpected(format!("activity start: {error}")))?
            .and_utc();
        Ok(Self {
            id: raw.activity_id,
            name: raw.activity_name.unwrap_or_default(),
            // The Personal API leaves hasPolyline out, but only sets a start with GPS.
            has_track: raw.has_polyline.unwrap_or(raw.start_latitude.is_some())
                && !raw.activity_type.type_key.starts_with("virtual"),
            activity_type: raw.activity_type.type_key,
            started_at,
            distance_m: raw.distance,
            duration_s: raw.duration,
            summary,
        })
    }
}

pub struct GarminClient {
    http: Client,
    base_url: String,
    api_key: String,
    /// Valid for 24 hours and never stored: a new one only costs one request.
    access_token: Option<String>,
}

impl GarminClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, GarminError> {
        let http = Client::builder()
            .user_agent(concat!("runmap/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key: api_key.trim().to_owned(),
            access_token: None,
        })
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
        Ok(response.bytes().await?.to_vec())
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

    async fn send(
        &mut self,
        build: impl Fn(&Client) -> RequestBuilder,
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
            StatusCode::TOO_MANY_REQUESTS => Err(GarminError::RateLimited),
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

    #[test]
    fn indoor_and_virtual_activities_have_no_track() {
        let activity = |type_key: &str, polyline: Value, latitude: Value| {
            Activity::from_summary(json!({
                "activityId": 1,
                "startTimeGMT": "2026-01-02 03:04:05",
                "activityType": {"typeKey": type_key},
                "hasPolyline": polyline,
                "startLatitude": latitude,
            }))
            .unwrap()
        };
        assert!(activity("running", json!(true), Value::Null).has_track);
        assert!(activity("cycling", Value::Null, json!(52.5)).has_track);
        assert!(!activity("strength_training", Value::Null, Value::Null).has_track);
        assert!(!activity("treadmill_running", json!(false), Value::Null).has_track);
        assert!(!activity("virtual_run", json!(true), json!(52.5)).has_track);
        assert!(!activity("virtual_ride", json!(true), json!(52.5)).has_track);
    }
}
