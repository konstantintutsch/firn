use std::time::Instant;

use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use serde::Deserialize;
use utoipa::IntoParams;

use crate::api::{ApiError, AppState, ErrorBody, require_header};
use crate::db::{self};
use crate::secret::{self};

use super::SESSION_COOKIE;
use super::oidc::{LOGIN_TIMEOUT, MAX_PENDING_LOGINS, PendingLogin};
use super::session::{SESSION_DAYS, cookie, redirect};

pub(super) const LOGIN_COOKIE: &str = "firn_login";

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

    let (user, _) = db::sign_in_user(
        &state.pool,
        &discovery.issuer,
        &claims.sub,
        email.as_deref(),
        name.as_deref(),
    )
    .await?;
    tracing::info!(user_id = user.id, "signed in");

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
