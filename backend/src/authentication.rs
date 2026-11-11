//! Sign-in through OpenID Connect, with the authorization code flow and PKCE.
//!
//! The ID token comes straight from the provider's token endpoint over TLS, which
//! OpenID Connect Core 3.1.3.7 accepts in place of checking its signature.
mod limit;
mod login;
mod oidc;
mod session;

pub use limit::limited;
pub(crate) use login::{LOGIN_COOKIE, begin_login, finish_login};
pub use oidc::{Auth, OidcSettings};
pub use session::{CurrentUser, SESSION_COOKIE};
pub(crate) use session::{SESSION_DAYS, cookie, redirect};
