//! Client for Garmin Connect's unofficial API, the one Garmin's own mobile apps use.
//!
//! Garmin offers no API for personal use, so these endpoints can change without notice.
//! Failed logins are never retried: repeating them gets an account locked out for days.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::{self, Pool};

const SSO_CLIENT_ID: &str = "GCM_IOS_DARK";
const SSO_SERVICE_URL: &str = "https://mobile.integration.garmin.com/gcm/ios";
const SSO_USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_7 like Mac OS X) \
                              AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";
const DI_GRANT_TYPE: &str =
    "https://connectapi.garmin.com/di-oauth2-service/oauth/grant/service_ticket";
const DI_CLIENT_IDS: [&str; 4] = [
    "GARMIN_CONNECT_MOBILE_ANDROID_DI_2025Q2",
    "GARMIN_CONNECT_MOBILE_ANDROID_DI_2024Q4",
    "GARMIN_CONNECT_MOBILE_ANDROID_DI",
    "GARMIN_CONNECT_MOBILE_IOS_DI",
];
const API_HEADERS: [(&str, &str); 7] = [
    ("user-agent", "GCM-Android-5.23"),
    (
        "x-garmin-user-agent",
        "com.garmin.android.apps.connectmobile/5.23; ; Google/sdk_gphone64_arm64/google; \
         Android/33; Dalvik/2.1.0",
    ),
    ("x-garmin-paired-app-version", "10861"),
    ("x-garmin-client-platform", "Android"),
    ("x-app-ver", "10861"),
    ("x-lang", "en"),
    ("x-gcexperience", "GC5"),
];
/// Refreshes this long before expiry, so a token cannot expire mid-request.
const EXPIRY_MARGIN_SECONDS: i64 = 900;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum GarminError {
    #[error("Garmin rate limited this client. Wait before trying again, possibly for a day")]
    RateLimited,
    #[error("Garmin blocked this client or network (HTTP 403)")]
    Blocked,
    #[error("Garmin asked for a CAPTCHA, which this client cannot solve")]
    CaptchaRequired,
    #[error("invalid Garmin email or password")]
    InvalidCredentials,
    #[error("wrong or expired MFA code")]
    InvalidMfaCode,
    #[error("the Garmin session is missing or expired. Run `just login`")]
    NeedsLogin,
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

#[derive(Clone)]
pub struct Endpoints {
    pub sso: String,
    pub di_token: String,
    pub api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            sso: "https://sso.garmin.com".into(),
            di_token: "https://diauth.garmin.com/di-oauth2-service/oauth/token".into(),
            api: "https://connectapi.garmin.com".into(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub client_id: String,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    response_status: Option<ResponseStatus>,
    service_ticket_id: Option<String>,
    customer_mfa_info: Option<MfaInfo>,
}

#[derive(Deserialize)]
struct ResponseStatus {
    r#type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MfaInfo {
    mfa_last_method_used: Option<String>,
}

/// Runs the mobile SSO login. `ask_mfa_code` is only called if the account uses MFA.
pub async fn login(
    endpoints: &Endpoints,
    email: &str,
    password: &str,
    ask_mfa_code: impl FnOnce() -> anyhow::Result<String>,
) -> Result<Tokens, GarminError> {
    let http = Client::builder()
        .cookie_store(true)
        .user_agent(SSO_USER_AGENT)
        .timeout(TIMEOUT)
        .build()?;
    let signin_url = format!("{}/mobile/sso/en_US/sign-in", endpoints.sso);
    let login_query = [
        ("clientId", SSO_CLIENT_ID),
        ("locale", "en-US"),
        ("service", SSO_SERVICE_URL),
    ];

    // The credentials are refused without the cookies this page sets.
    let page = http
        .get(&signin_url)
        .query(&[("clientId", SSO_CLIENT_ID), ("service", SSO_SERVICE_URL)])
        .header("accept", "text/html,application/xhtml+xml,*/*;q=0.8")
        .send()
        .await?;
    check_login_status(page.status())?;

    let referer = format!("{signin_url}?clientId={SSO_CLIENT_ID}");
    let submit = |path: &str, body: Value| {
        http.post(format!("{}/mobile/api/{path}", endpoints.sso))
            .query(&login_query)
            .header("accept", "application/json, text/plain, */*")
            .header("origin", &endpoints.sso)
            .header("referer", &referer)
            .json(&body)
    };

    let mut response = parse_login(
        submit(
            "login",
            json!({
                "username": email,
                "password": password,
                "rememberMe": true,
                "captchaToken": "",
            }),
        )
        .send()
        .await?,
    )
    .await?;

    if login_type(&response) == "MFA_REQUIRED" {
        let method = response
            .customer_mfa_info
            .and_then(|info| info.mfa_last_method_used)
            .unwrap_or_else(|| "email".into());
        let code = ask_mfa_code().map_err(GarminError::Database)?;
        response = parse_login(
            submit(
                "mfa/verifyCode",
                json!({
                    "mfaMethod": method,
                    "mfaVerificationCode": code,
                    "rememberMyBrowser": true,
                    "reconsentList": [],
                    "mfaSetup": false,
                }),
            )
            .send()
            .await?,
        )
        .await?;
        if login_type(&response) != "SUCCESSFUL" {
            return Err(GarminError::InvalidMfaCode);
        }
    }

    match login_type(&response) {
        "SUCCESSFUL" => {}
        "INVALID_USERNAME_PASSWORD" => return Err(GarminError::InvalidCredentials),
        "CAPTCHA_REQUIRED" => return Err(GarminError::CaptchaRequired),
        // Only the status: the body can carry account data.
        other => return Err(GarminError::Unexpected(format!("login status {other:?}"))),
    }
    let ticket = response
        .service_ticket_id
        .ok_or_else(|| GarminError::Unexpected("login without service ticket".into()))?;

    let tokens = exchange_ticket(endpoints, &ticket).await?;
    verify_tokens(endpoints, &tokens).await?;
    Ok(tokens)
}

fn login_type(response: &LoginResponse) -> &str {
    response
        .response_status
        .as_ref()
        .map_or("unknown", |status| status.r#type.as_str())
}

fn check_login_status(status: StatusCode) -> Result<(), GarminError> {
    match status.as_u16() {
        427 | 429 => Err(GarminError::RateLimited),
        403 => Err(GarminError::Blocked),
        _ => Ok(()),
    }
}

async fn parse_login(response: Response) -> Result<LoginResponse, GarminError> {
    let status = response.status();
    check_login_status(status)?;
    response
        .json()
        .await
        .map_err(|_| GarminError::Unexpected(format!("non-JSON login answer, HTTP {status}")))
}

async fn exchange_ticket(endpoints: &Endpoints, ticket: &str) -> Result<Tokens, GarminError> {
    let http = api_client()?;
    for client_id in DI_CLIENT_IDS {
        let response = http
            .post(&endpoints.di_token)
            .basic_auth(client_id, Some(""))
            .header("accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("service_ticket", ticket),
                ("grant_type", DI_GRANT_TYPE),
                ("service_url", SSO_SERVICE_URL),
            ])
            .send()
            .await?;
        check_login_status(response.status())?;
        if !response.status().is_success() {
            continue;
        }
        let body: TokenResponse = response.json().await?;
        return Ok(Tokens {
            client_id: jwt_claim(&body.access_token, "client_id")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| client_id.to_owned()),
            refresh_token: body
                .refresh_token
                .ok_or_else(|| GarminError::Unexpected("no refresh token issued".into()))?,
            access_token: body.access_token,
        });
    }
    Err(GarminError::Unexpected(
        "token exchange failed for all client IDs".into(),
    ))
}

/// The token host can issue tokens that the API tier refuses, depending on the account.
async fn verify_tokens(endpoints: &Endpoints, tokens: &Tokens) -> Result<(), GarminError> {
    let response = api_client()?
        .get(format!(
            "{}/userprofile-service/socialProfile",
            endpoints.api
        ))
        .bearer_auth(&tokens.access_token)
        .send()
        .await?;
    match response.status() {
        StatusCode::UNAUTHORIZED => Err(GarminError::Unexpected(
            "the API refused the newly issued token".into(),
        )),
        status => check_login_status(status),
    }
}

fn api_client() -> Result<Client, GarminError> {
    let mut headers = HeaderMap::new();
    for (name, value) in API_HEADERS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    Ok(Client::builder()
        .default_headers(headers)
        .timeout(TIMEOUT)
        .build()?)
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
    /// False for treadmill and indoor runs. Virtual runs also count as false:
    /// their route is in a virtual world, not where the runner was.
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
            has_track: raw.has_polyline != Some(false)
                && raw.activity_type.type_key != "virtual_run",
            activity_type: raw.activity_type.type_key,
            started_at,
            distance_m: raw.distance,
            duration_s: raw.duration,
            summary,
        })
    }
}

/// An authenticated API session. Refreshed tokens are saved right away, because
/// Garmin may invalidate the previous refresh token.
pub struct GarminClient {
    http: Client,
    endpoints: Endpoints,
    tokens: Tokens,
    pool: Pool,
}

impl GarminClient {
    pub async fn from_saved_tokens(pool: Pool, endpoints: Endpoints) -> Result<Self, GarminError> {
        let tokens = db::load_tokens(&pool)
            .await?
            .ok_or(GarminError::NeedsLogin)?;
        Ok(Self {
            http: api_client()?,
            endpoints,
            tokens,
            pool,
        })
    }

    /// One page of running activities, newest first. Includes all running subtypes.
    pub async fn list_runs(
        &mut self,
        start: u32,
        limit: u32,
    ) -> Result<Vec<Activity>, GarminError> {
        let url = format!(
            "{}/activitylist-service/activities/search/activities",
            self.endpoints.api
        );
        let query = [
            ("start", start.to_string()),
            ("limit", limit.to_string()),
            ("activityType", "running".to_owned()),
        ];
        let response = self
            .send(|http| {
                http.get(&url)
                    .query(&query)
                    .header("accept", "application/json")
            })
            .await?;
        let summaries: Vec<Value> = response.json().await?;
        summaries.into_iter().map(Activity::from_summary).collect()
    }

    pub async fn download_gpx(&mut self, activity_id: i64) -> Result<String, GarminError> {
        let url = format!(
            "{}/download-service/export/gpx/activity/{activity_id}",
            self.endpoints.api
        );
        let response = self
            .send(|http| http.get(&url).header("accept", "*/*"))
            .await?;
        Ok(response.text().await?)
    }

    async fn send(
        &mut self,
        build: impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Response, GarminError> {
        if expires_soon(&self.tokens.access_token, Utc::now()) {
            self.refresh().await?;
        }
        let mut response = build(&self.http)
            .bearer_auth(&self.tokens.access_token)
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh().await?;
            response = build(&self.http)
                .bearer_auth(&self.tokens.access_token)
                .send()
                .await?;
        }
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::UNAUTHORIZED => Err(GarminError::NeedsLogin),
            StatusCode::NOT_FOUND => Err(GarminError::NotFound),
            status => {
                check_login_status(status)?;
                Err(GarminError::Status(status))
            }
        }
    }

    async fn refresh(&mut self) -> Result<(), GarminError> {
        let client_id = self.tokens.client_id.clone();
        let response = self
            .http
            .post(&self.endpoints.di_token)
            .basic_auth(&client_id, Some(""))
            .header("accept", "application/json")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &client_id),
                ("refresh_token", &self.tokens.refresh_token),
            ])
            .send()
            .await?;
        let status = response.status();
        check_login_status(status)?;
        if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
            return Err(GarminError::NeedsLogin);
        }
        if !status.is_success() {
            return Err(GarminError::Status(status));
        }
        let body: TokenResponse = response.json().await?;
        if let Some(refresh_token) = body.refresh_token {
            self.tokens.refresh_token = refresh_token;
        }
        self.tokens.access_token = body.access_token;
        db::save_tokens(&self.pool, &self.tokens).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(payload: Value) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload.to_string()))
    }

    #[test]
    fn expiry_respects_margin() {
        let now = DateTime::from_timestamp(10_000, 0).unwrap();
        assert!(expires_soon(&jwt(json!({"exp": 10_000})), now));
        assert!(expires_soon(&jwt(json!({"exp": 10_899})), now));
        assert!(!expires_soon(&jwt(json!({"exp": 10_901})), now));
    }

    #[test]
    fn opaque_token_is_not_treated_as_expiring() {
        assert!(!expires_soon("opaque", Utc::now()));
        assert!(!expires_soon(&jwt(json!({"exp": "soon"})), Utc::now()));
    }

    #[test]
    fn token_debug_hides_secrets() {
        let tokens = Tokens {
            access_token: "secret-a".into(),
            refresh_token: "secret-r".into(),
            client_id: "id".into(),
        };
        assert!(!format!("{tokens:?}").contains("secret"));
    }

    #[test]
    fn indoor_and_virtual_runs_have_no_track() {
        let activity = |type_key: &str, polyline: Value| {
            Activity::from_summary(json!({
                "activityId": 1,
                "startTimeGMT": "2026-01-02 03:04:05",
                "activityType": {"typeKey": type_key},
                "hasPolyline": polyline,
            }))
            .unwrap()
        };
        assert!(activity("running", json!(true)).has_track);
        assert!(activity("running", Value::Null).has_track);
        assert!(!activity("treadmill_running", json!(false)).has_track);
        assert!(!activity("virtual_run", json!(true)).has_track);
    }
}
