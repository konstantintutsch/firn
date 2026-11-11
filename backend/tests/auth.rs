mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, Response, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use firn::api::{AppState, router};
use firn::auth::OidcSettings;
use firn::database;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT_ID: &str = "firn";
const TOKEN: &str = "/personalapitoken-service/personal-api/personal-api-key/token";

async fn send(state: &AppState, mut request: Request<Body>) -> Response<Body> {
    // The address of the connection, which the limit on sign-ins asks for.
    let address = SocketAddr::from(([127, 0, 0, 1], 0));
    request.extensions_mut().insert(ConnectInfo(address));
    router(state.clone()).oneshot(request).await.unwrap()
}

async fn json_body(response: Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn header_value<'a>(response: &'a Response<Body>, name: &str) -> &'a str {
    response.headers()[name].to_str().unwrap()
}

/// The `name=value` parts of all cookies a response sets.
fn set_cookies(response: &Response<Body>) -> Vec<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn query_value(url: &str, name: &str) -> String {
    reqwest::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
        .into_owned()
}

fn unsigned_jwt(claims: Value) -> String {
    format!(
        "{}.{}.",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

struct Provider {
    server: MockServer,
    issuer: String,
}

async fn provider() -> Provider {
    let server = MockServer::start().await;
    let issuer = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "userinfo_endpoint": format!("{issuer}/userinfo"),
        })))
        .mount(&server)
        .await;
    Provider { server, issuer }
}

impl Provider {
    async fn answer_token(&self, claims: Value) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(header("authorization", "Basic ZmlybjpzZWNyZXQ="))
            .and(body_string_contains("code=the-code"))
            .and(body_string_contains("code_verifier="))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "access",
                "token_type": "Bearer",
                "id_token": unsigned_jwt(claims),
            })))
            .mount(&self.server)
            .await;
    }

    fn claims(&self, nonce: &str) -> Value {
        json!({
            "iss": self.issuer,
            "sub": "person-1",
            "aud": CLIENT_ID,
            "exp": chrono::Utc::now().timestamp() + 300,
            "nonce": nonce,
            "email": "person@example.com",
            "name": "Person",
        })
    }
}

async fn app(provider: &Provider, garmin: &str) -> AppState {
    let pool = common::test_pool().await;
    common::state_with_oidc(
        pool,
        garmin,
        None,
        Some(OidcSettings {
            issuer: provider.issuer.clone(),
            client_id: CLIENT_ID.to_owned(),
            client_secret: Some("secret".to_owned()),
            public_url: "https://firn.example".to_owned(),
            extra_roots: Vec::new(),
        }),
    )
    .await
}

/// Starts a login and returns the state, the nonce and the login cookie.
async fn start_login(state: &AppState) -> (String, String, String) {
    let response = send(
        state,
        Request::get("/api/auth/login").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = header_value(&response, "location").to_owned();
    assert!(location.contains("/authorize?"));
    assert_eq!(query_value(&location, "code_challenge_method"), "S256");
    assert_eq!(
        query_value(&location, "redirect_uri"),
        "https://firn.example/api/auth/callback"
    );
    let set_cookie = header_value(&response, "set-cookie");
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("Secure"));
    (
        query_value(&location, "state"),
        query_value(&location, "nonce"),
        set_cookies(&response).remove(0),
    )
}

async fn callback(state: &AppState, login_state: &str, cookie: &str) -> Response<Body> {
    let request = Request::get(format!(
        "/api/auth/callback?code=the-code&state={login_state}"
    ))
    .header("cookie", cookie)
    .body(Body::empty())
    .unwrap();
    send(state, request).await
}

#[tokio::test]
async fn signs_in_through_the_provider() {
    let provider = provider().await;
    let state = app(&provider, "http://unused").await;
    let (login_state, nonce, cookie) = start_login(&state).await;
    provider.answer_token(provider.claims(&nonce)).await;

    let response = callback(&state, &login_state, &cookie).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(header_value(&response, "location"), "/");
    let me = me_with(&state, &response).await;
    assert_eq!(me.status(), StatusCode::OK);
    let me = json_body(me).await;
    assert_eq!(me["email"], "person@example.com");
    assert_eq!(me["name"], "Person");
    assert_eq!(me["has_garmin_key"], false);
}

#[tokio::test]
async fn reads_the_profile_from_user_info_if_the_token_has_none() {
    let provider = provider().await;
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .and(header("authorization", "Bearer access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sub": "person-1",
            "email": "info@example.com",
            "preferred_username": "info",
        })))
        .expect(1)
        .mount(&provider.server)
        .await;
    let state = app(&provider, "http://unused").await;
    let (login_state, nonce, cookie) = start_login(&state).await;
    let mut claims = provider.claims(&nonce);
    claims.as_object_mut().unwrap().remove("email");
    claims.as_object_mut().unwrap().remove("name");
    provider.answer_token(claims).await;

    let response = callback(&state, &login_state, &cookie).await;
    assert_eq!(header_value(&response, "location"), "/");
    let me = json_body(me_with(&state, &response).await).await;
    assert_eq!(me["email"], "info@example.com");
    assert_eq!(me["name"], "info");
}

/// Asks for the account with the session that a callback response started.
async fn me_with(state: &AppState, callback: &Response<Body>) -> Response<Body> {
    let session = set_cookies(callback)
        .into_iter()
        .find(|cookie| cookie.starts_with("firn_session=") && cookie.len() > 20)
        .unwrap();
    send(
        state,
        Request::get("/api/me")
            .header("cookie", &session)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

#[tokio::test]
async fn refuses_a_token_with_another_nonce() {
    let provider = provider().await;
    let state = app(&provider, "http://unused").await;
    let (login_state, _, cookie) = start_login(&state).await;
    provider.answer_token(provider.claims("other")).await;

    let response = callback(&state, &login_state, &cookie).await;
    assert_eq!(header_value(&response, "location"), "/?login=failed");
    assert!(
        !set_cookies(&response)
            .iter()
            .any(|cookie| cookie.starts_with("firn_session="))
    );
}

#[tokio::test]
async fn refuses_a_token_for_another_client() {
    let provider = provider().await;
    let state = app(&provider, "http://unused").await;
    let (login_state, nonce, cookie) = start_login(&state).await;
    let mut claims = provider.claims(&nonce);
    claims["aud"] = json!("someone-else");
    provider.answer_token(claims).await;

    let response = callback(&state, &login_state, &cookie).await;
    assert_eq!(header_value(&response, "location"), "/?login=failed");
}

#[tokio::test]
async fn refuses_a_callback_from_another_browser() {
    let provider = provider().await;
    let state = app(&provider, "http://unused").await;
    let (login_state, nonce, _) = start_login(&state).await;
    provider.answer_token(provider.claims(&nonce)).await;

    let response = callback(&state, &login_state, "firn_login=other").await;
    assert_eq!(header_value(&response, "location"), "/?login=failed");
    // The state was not used up, so the right browser can still finish.
    assert!(
        provider
            .server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() != "/token")
    );
}

#[tokio::test]
async fn state_works_only_once() {
    let provider = provider().await;
    let state = app(&provider, "http://unused").await;
    let (login_state, nonce, cookie) = start_login(&state).await;
    provider.answer_token(provider.claims(&nonce)).await;

    callback(&state, &login_state, &cookie).await;
    let again = callback(&state, &login_state, &cookie).await;
    assert_eq!(header_value(&again, "location"), "/?login=failed");
}

#[tokio::test]
async fn login_without_oidc_explains_what_to_set() {
    let pool = common::test_pool().await;
    let state = common::state(pool, "http://unused", None).await;
    let response = send(
        &state,
        Request::get("/api/auth/login").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        json_body(response).await["error"]
            .as_str()
            .unwrap()
            .contains("OIDC_ISSUER")
    );
}

#[tokio::test]
async fn data_needs_a_session() {
    let pool = common::test_pool().await;
    let state = common::state(pool, "http://unused", None).await;
    for uri in [
        "/api/me",
        "/api/stats",
        "/api/activities",
        &format!("/api/activities/{}", common::uuid(1)),
        "/api/activity-types",
        "/api/sync",
        "/api/heatmap?zoom=1&west=-180&south=-90&east=180&north=90",
        "/api/statistics/activities",
        "/api/statistics/health",
    ] {
        let response = send(
            &state,
            Request::get(uri)
                .header("cookie", "firn_session=made-up")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn logout_ends_the_session() {
    let pool = common::test_pool().await;
    let cookie = common::cookie(&pool).await;
    let state = common::state(pool, "http://unused", None).await;
    let me = |cookie: String| {
        Request::get("/api/me")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        send(&state, me(cookie.clone())).await.status(),
        StatusCode::OK
    );

    let logout = Request::post("/api/auth/logout")
        .header("cookie", &cookie)
        .header("x-requested-with", "firn")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&state, logout).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        send(&state, me(cookie)).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

fn put_key(cookie: &str, key: &str) -> Request<Body> {
    Request::put("/api/me/garmin-key")
        .header("cookie", cookie)
        .header("x-requested-with", "firn")
        .header("content-type", "application/json")
        .body(Body::from(json!({"api_key": key}).to_string()))
        .unwrap()
}

#[tokio::test]
async fn saves_a_garmin_key_that_garmin_accepts_encrypted() {
    let garmin = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .and(header("authorization", "ApiKey good-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string("access"))
        .mount(&garmin)
        .await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(401))
        .mount(&garmin)
        .await;
    let pool = common::test_pool().await;
    let cookie = common::cookie(&pool).await;
    let state = common::state(pool.clone(), &garmin.uri(), None).await;

    let refused = send(&state, put_key(&cookie, "bad-key")).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    let saved = send(&state, put_key(&cookie, "good-key")).await;
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);
    let me = send(
        &state,
        Request::get("/api/me")
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let me = json_body(me).await;
    assert_eq!(me["has_garmin_key"], true);
    assert!(!me.to_string().contains("good-key"));

    let (user, _) = database::sign_in_user(&pool, common::ISSUER, "tester", None, None)
        .await
        .unwrap();
    let sealed = user.garmin_api_key.unwrap();
    assert!(!sealed.windows(8).any(|window| window == b"good-key"));
    assert_eq!(common::secret_box().open(&sealed).unwrap(), "good-key");
}

#[tokio::test]
async fn key_changes_need_the_header_that_other_websites_cannot_send() {
    let pool = common::test_pool().await;
    let cookie = common::cookie(&pool).await;
    let state = common::state(pool, "http://unused", None).await;
    let request = Request::delete("/api/me/garmin-key")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&state, request).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn first_account_claims_activities_from_before_accounts() {
    let pool = common::test_pool().await;
    {
        use diesel_async::RunQueryDsl;
        diesel::sql_query(
            "INSERT INTO activities (garmin_id, name, activity_type, started_at, summary) \
             VALUES (7, 'Old run', 'running', now(), '{}')",
        )
        .execute(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    }
    let first = common::user_named(&pool, "first").await;
    let second = common::user_named(&pool, "second").await;
    let owned = |user_id| {
        let pool = pool.clone();
        async move {
            let filter = firn::models::Filter {
                user_id,
                without_gps: true,
                ..Default::default()
            };
            database::activity_records(&pool, &filter)
                .await
                .unwrap()
                .len()
        }
    };
    assert_eq!(owned(first).await, 1);
    assert_eq!(owned(second).await, 0);
}

#[tokio::test]
async fn expired_sessions_stop_working_and_are_removed() {
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use firn::schema::sessions;

    let pool = common::test_pool().await;
    let user_id = common::user(&pool).await;
    let token = firn::secret::random_token();
    database::create_session(
        &pool,
        user_id,
        &firn::secret::sha256(&token),
        chrono::Utc::now() - chrono::Duration::minutes(1),
    )
    .await
    .unwrap();
    let state = common::state(pool.clone(), "http://unused", None).await;
    let me = Request::get("/api/me")
        .header("cookie", format!("firn_session={token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&state, me).await.status(), StatusCode::UNAUTHORIZED);

    common::cookie(&pool).await;
    let remaining: i64 = sessions::table
        .count()
        .get_result(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn limits_the_sign_ins_of_each_client() {
    let provider = provider().await;
    let state = app(&provider, "http://garmin.invalid").await;
    let app = router(state);
    let logout = |forwarded: &'static str| {
        let request = Request::post("/api/auth/logout")
            .header("x-forwarded-for", forwarded)
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(request)
    };

    for _ in 0..20 {
        let response = logout("203.0.113.1").await.unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
    let response = logout("203.0.113.1").await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key("retry-after"));
    assert!(json_body(response).await["error"].is_string());

    let response = logout("203.0.113.2").await.unwrap();
    assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}
