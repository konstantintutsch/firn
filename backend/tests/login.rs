mod common;

use runmap::garmin::{self, GarminError};
use serde_json::json;
use wiremock::matchers::{body_partial_json, body_string_contains, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN: &str = "/mobile/api/login";

async fn server_with_signin_page() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mobile/sso/en_US/sign-in"))
        .respond_with(ResponseTemplate::new(200).insert_header("set-cookie", "SESSION=abc; Path=/"))
        .mount(&server)
        .await;
    server
}

async fn mount_token_exchange(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("service_ticket=ST-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "access", "refresh_token": "refresh"})),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/userprofile-service/socialProfile"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
}

fn no_mfa() -> anyhow::Result<String> {
    panic!("MFA was not expected")
}

#[tokio::test]
async fn logs_in_with_cookies_from_the_signin_page() {
    let server = server_with_signin_page().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .and(wiremock::matchers::header("cookie", "SESSION=abc"))
        .and(body_partial_json(
            json!({"username": "a@b.c", "password": "pw"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "SUCCESSFUL"},
            "serviceTicketId": "ST-1",
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_token_exchange(&server).await;

    let tokens = garmin::login(&common::endpoints(&server), "a@b.c", "pw", no_mfa)
        .await
        .unwrap();
    assert_eq!(tokens.access_token, "access");
    assert_eq!(tokens.refresh_token, "refresh");
}

#[tokio::test]
async fn completes_mfa() {
    let server = server_with_signin_page().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "MFA_REQUIRED"},
            "customerMfaInfo": {"mfaLastMethodUsed": "totp"},
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/mfa/verifyCode"))
        .and(body_partial_json(
            json!({"mfaMethod": "totp", "mfaVerificationCode": "123456"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "SUCCESSFUL"},
            "serviceTicketId": "ST-1",
        })))
        .expect(1)
        .mount(&server)
        .await;
    mount_token_exchange(&server).await;

    let tokens = garmin::login(&common::endpoints(&server), "a@b.c", "pw", || {
        Ok("123456".to_owned())
    })
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "access");
}

#[tokio::test]
async fn wrong_mfa_code_is_reported() {
    let server = server_with_signin_page().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "MFA_REQUIRED"},
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mobile/api/mfa/verifyCode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "INVALID_MFA_CODE"},
        })))
        .mount(&server)
        .await;

    let result = garmin::login(
        &common::endpoints(&server),
        "a@b.c",
        "pw",
        || Ok("0".into()),
    )
    .await;
    assert!(matches!(result, Err(GarminError::InvalidMfaCode)));
}

#[tokio::test]
async fn wrong_password_is_reported() {
    let server = server_with_signin_page().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "responseStatus": {"type": "INVALID_USERNAME_PASSWORD"},
        })))
        .mount(&server)
        .await;

    let result = garmin::login(&common::endpoints(&server), "a@b.c", "pw", no_mfa).await;
    assert!(matches!(result, Err(GarminError::InvalidCredentials)));
}

#[tokio::test]
async fn rate_limited_login_is_not_retried() {
    let server = server_with_signin_page().await;
    Mock::given(method("POST"))
        .and(path(LOGIN))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;

    let result = garmin::login(&common::endpoints(&server), "a@b.c", "pw", no_mfa).await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
}
