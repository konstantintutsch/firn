use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderValue;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::secret::SecretBox;

/// How long a person has to sign in at the provider.
pub(super) const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
/// Sign-ins that can be under way at once, so that repeated calls cannot fill the memory.
pub(super) const MAX_PENDING_LOGINS: usize = 1000;
pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

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
    pub(super) fn redirect_url(&self) -> String {
        format!(
            "{}/api/auth/callback",
            self.public_url.trim_end_matches('/')
        )
    }

    /// Browsers drop secure cookies on plain HTTP, which localhost may use.
    pub(super) fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

#[derive(Deserialize)]
pub(super) struct Discovery {
    pub(super) issuer: String,
    pub(super) authorization_endpoint: String,
    pub(super) token_endpoint: String,
    pub(super) userinfo_endpoint: Option<String>,
}

pub(super) struct PendingLogin {
    pub(super) nonce: String,
    pub(super) verifier: String,
    pub(super) started: Instant,
}

pub struct Auth {
    pub(super) oidc: Option<OidcSettings>,
    pub(super) http: Client,
    pub(super) discovery: OnceCell<Discovery>,
    pub(super) pending: Mutex<HashMap<String, PendingLogin>>,
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

    pub(super) fn secure_cookies(&self) -> bool {
        self.oidc.as_ref().is_some_and(OidcSettings::secure_cookies)
    }

    pub(super) async fn discovery(&self, oidc: &OidcSettings) -> anyhow::Result<&Discovery> {
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

    pub(crate) fn cookie(&self, name: &str, value: &str, max_age: i64, path: &str) -> HeaderValue {
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
