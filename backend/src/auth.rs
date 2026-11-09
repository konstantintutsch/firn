//! Sign-in through OpenID Connect, with the authorization code flow and PKCE.
//!
//! The ID token comes straight from the provider's token endpoint over TLS, which
//! OpenID Connect Core 3.1.3.7 accepts in place of checking its signature.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::Json;
use axum::extract::{FromRequestParts, Query, State};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use utoipa::{IntoParams, ToSchema};

use crate::api::{ApiError, AppState, ErrorBody, require_header};
use crate::db::{self, User};
use crate::garmin::{GarminClient, GarminError};
use crate::secret::{self, SecretBox};

pub const SESSION_COOKIE: &str = "firn_session";
const LOGIN_COOKIE: &str = "firn_login";
const SESSION_DAYS: i64 = 30;
/// How long a person has to sign in at the provider.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// Sign-ins that can be under way at once, so that repeated calls cannot fill the memory.
const MAX_PENDING_LOGINS: usize = 1000;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct OidcSettings {
    /// Like `https://auth.example.com/application/o/firn/`.
    pub issuer: String,
    pub client_id: String,
    /// `None` for a public client, which only proves itself with PKCE.
    pub client_secret: Option<String>,
    /// The address of the app, like `https://firn.example.com`.
    pub public_url: String,
    /// Certificates to trust besides the system's, for a provider with its own CA.
    pub extra_roots: Vec<reqwest::Certificate>,
}

impl OidcSettings {
    fn redirect_url(&self) -> String {
        format!(
            "{}/api/auth/callback",
            self.public_url.trim_end_matches('/')
        )
    }

    /// Browsers drop secure cookies on plain HTTP, which localhost may use.
    fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
}

struct PendingLogin {
    nonce: String,
    verifier: String,
    started: Instant,
}

pub struct Auth {
    oidc: Option<OidcSettings>,
    http: Client,
    discovery: OnceCell<Discovery>,
    pending: Mutex<HashMap<String, PendingLogin>>,
    pub secret_box: SecretBox,
}

impl Auth {
    pub fn new(oidc: Option<OidcSettings>, secret_box: SecretBox) -> Self {
        let roots = oidc
            .as_ref()
            .map(|oidc| oidc.extra_roots.clone())
            .unwrap_or_default();
        let http = Client::builder()
            .user_agent(concat!("firn/", env!("CARGO_PKG_VERSION")))
            .timeout(TIMEOUT)
            .tls_certs_merge(roots)
            .build()
            .expect("default TLS backend is available");
        Self {
            oidc,
            http,
            discovery: OnceCell::new(),
            pending: Mutex::default(),
            secret_box,
        }
    }

    fn secure_cookies(&self) -> bool {
        self.oidc.as_ref().is_some_and(OidcSettings::secure_cookies)
    }

    async fn discovery(&self, oidc: &OidcSettings) -> anyhow::Result<&Discovery> {
        self.discovery
            .get_or_try_init(async || {
                let issuer = oidc.issuer.trim_end_matches('/');
                let discovery: Discovery = self
                    .http
                    .get(format!("{issuer}/.well-known/openid-configuration"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                if discovery.issuer.trim_end_matches('/') != issuer {
                    anyhow::bail!("the provider names another issuer: {}", discovery.issuer);
                }
                Ok(discovery)
            })
            .await
    }

    fn cookie(&self, name: &str, value: &str, max_age: i64, path: &str) -> HeaderValue {
        let secure = if self.secure_cookies() {
            "; Secure"
        } else {
            ""
        };
        HeaderValue::from_str(&format!(
            "{name}={value}; Max-Age={max_age}; Path={path}; HttpOnly; SameSite=Lax{secure}"
        ))
        .expect("cookie values are URL-safe")
    }
}

/// The signed-in account. Answers 401 without a valid session.
pub struct CurrentUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
        db::session_user(&state.pool, &secret::sha256(token))
            .await?
            .map(CurrentUser)
            .ok_or(ApiError::Unauthorized)
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
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

fn redirect(location: &str, cookies: Vec<HeaderValue>) -> Response {
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
pub async fn login(State(state): State<AppState>) -> Result<Response, ApiError> {
    let auth = &state.auth;
    let Some(oidc) = &auth.oidc else {
        return Err(ApiError::Conflict(
            "OpenID Connect is not set up. Set OIDC_ISSUER and OIDC_CLIENT_ID in .env",
        ));
    };
    let discovery = auth.discovery(oidc).await?;

    let login_state = secret::random_token();
    let nonce = secret::random_token();
    let verifier = secret::random_token();
    let challenge = secret::code_challenge(&verifier);
    {
        let mut pending = auth.pending.lock().unwrap();
        pending.retain(|_, login| login.started.elapsed() < LOGIN_TIMEOUT);
        if pending.len() >= MAX_PENDING_LOGINS {
            return Err(ApiError::TooManyRequests(
                "too many sign-ins at once. Try again in a few minutes",
            ));
        }
        pending.insert(
            login_state.clone(),
            PendingLogin {
                nonce: nonce.clone(),
                verifier,
                started: Instant::now(),
            },
        );
    }

    let mut url = reqwest::Url::parse(&discovery.authorization_endpoint)
        .map_err(|error| anyhow::anyhow!("invalid authorization endpoint: {error}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.client_id)
        .append_pair("redirect_uri", &oidc.redirect_url())
        .append_pair("scope", "openid email profile")
        .append_pair("state", &login_state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    // Ties the login to this browser, so that a link from someone else cannot finish it.
    let cookie = auth.cookie(
        LOGIN_COOKIE,
        &login_state,
        LOGIN_TIMEOUT.as_secs() as i64,
        "/api/auth",
    );
    Ok(redirect(url.as_str(), vec![cookie]))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    /// Set by the provider if the person did not sign in.
    error: Option<String>,
}

/// Where the provider sends the browser back to. Starts a session and opens the app.
#[utoipa::path(
    get,
    path = "/api/auth/callback",
    params(CallbackQuery),
    responses((status = 303, description = "Opens the app, with `?login=failed` on failure"))
)]
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let clear_login = state.auth.cookie(LOGIN_COOKIE, "", 0, "/api/auth");
    match finish_login(&state, &headers, query).await {
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

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: Option<String>,
}

/// The profile claims, which some providers like Authelia only return here.
#[derive(Deserialize)]
struct UserInfo {
    sub: String,
    email: Option<String>,
    name: Option<String>,
    preferred_username: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: i64,
    nonce: Option<String>,
    azp: Option<String>,
    email: Option<String>,
    name: Option<String>,
    preferred_username: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, client_id: &str) -> bool {
        match self {
            Audience::One(audience) => audience == client_id,
            Audience::Many(audiences) => audiences.iter().any(|audience| audience == client_id),
        }
    }

    fn is_many(&self) -> bool {
        matches!(self, Audience::Many(audiences) if audiences.len() > 1)
    }
}

/// Returns the token of the new session.
async fn finish_login(
    state: &AppState,
    headers: &HeaderMap,
    query: CallbackQuery,
) -> anyhow::Result<String> {
    let auth = &state.auth;
    let oidc = auth.oidc.as_ref().context("OpenID Connect is not set up")?;
    if let Some(error) = query.error {
        anyhow::bail!("the provider answered {error}");
    }
    let code = query.code.context("no code")?;
    let login_state = query.state.context("no state")?;
    if cookie(headers, LOGIN_COOKIE) != Some(login_state.as_str()) {
        anyhow::bail!("the state does not belong to this browser");
    }
    let pending = auth
        .pending
        .lock()
        .unwrap()
        .remove(&login_state)
        .filter(|login| login.started.elapsed() < LOGIN_TIMEOUT)
        .context("unknown or expired state")?;
    let discovery = auth.discovery(oidc).await?;

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("code_verifier", pending.verifier.as_str()),
    ];
    let redirect_url = oidc.redirect_url();
    form.push(("redirect_uri", &redirect_url));
    let mut request = auth.http.post(&discovery.token_endpoint);
    match &oidc.client_secret {
        Some(secret) => request = request.basic_auth(&oidc.client_id, Some(secret)),
        None => form.push(("client_id", &oidc.client_id)),
    }
    let response = request.form(&form).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("the token endpoint answered HTTP {}", response.status());
    }
    let tokens: TokenResponse = response.json().await?;
    let claims = id_token_claims(&tokens.id_token)?;

    if claims.iss.trim_end_matches('/') != discovery.issuer.trim_end_matches('/') {
        anyhow::bail!("the ID token names another issuer");
    }
    if !claims.aud.contains(&oidc.client_id) {
        anyhow::bail!("the ID token is for another client");
    }
    if claims.aud.is_many() && claims.azp.as_deref() != Some(oidc.client_id.as_str()) {
        anyhow::bail!("the ID token was issued to another client");
    }
    if claims.exp <= Utc::now().timestamp() {
        anyhow::bail!("the ID token has expired");
    }
    if claims.nonce.as_deref() != Some(pending.nonce.as_str()) {
        anyhow::bail!("the ID token has another nonce");
    }

    let mut email = claims.email;
    let mut name = claims.name.or(claims.preferred_username);
    if (email.is_none() || name.is_none())
        && let (Some(endpoint), Some(access_token)) =
            (&discovery.userinfo_endpoint, &tokens.access_token)
    {
        let info: UserInfo = auth
            .http
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if info.sub != claims.sub {
            anyhow::bail!("the user info is about someone else");
        }
        email = email.or(info.email);
        name = name.or(info.name).or(info.preferred_username);
    }

    let (user, is_first) = db::sign_in_user(
        &state.pool,
        &discovery.issuer,
        &claims.sub,
        email.as_deref(),
        name.as_deref(),
    )
    .await?;
    tracing::info!(user_id = user.id, "signed in");
    // Keeps a setup from before accounts working without entering the key again.
    if is_first
        && user.garmin_api_key.is_none()
        && let Some(key) = &state.garmin.api_key
    {
        db::set_garmin_key(&state.pool, user.id, Some(&auth.secret_box.seal(key)?)).await?;
    }

    let token = secret::random_token();
    let expires = Utc::now() + chrono::Duration::days(SESSION_DAYS);
    db::create_session(&state.pool, user.id, &secret::sha256(&token), expires).await?;
    Ok(token)
}

fn id_token_claims(id_token: &str) -> anyhow::Result<Claims> {
    let payload = id_token.split('.').nth(1).context("malformed ID token")?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("malformed ID token")?;
    Ok(serde_json::from_slice(&payload)?)
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
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_header(&headers)?;
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        db::delete_session(&state.pool, &secret::sha256(token)).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, state.auth.cookie(SESSION_COOKIE, "", 0, "/"));
    Ok(response)
}

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
    for id in db::activity_ids(&state.pool, user.id).await? {
        state.files.remove_activity(id).await?;
    }
    db::delete_user(&state.pool, user.id).await?;
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
    db::set_garmin_key(
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
    db::set_garmin_key(&state.pool, user.id, None).await?;
    // Without the key, a running sync should not keep using it.
    state.cancel_sync(user.id);
    Ok(StatusCode::NO_CONTENT)
}
