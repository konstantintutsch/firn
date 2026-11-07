#![allow(dead_code)]

use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use runmap::db::{self, Pool};
use runmap::garmin::{Endpoints, Tokens};
use std::sync::atomic::{AtomicU32, Ordering};
use wiremock::MockServer;

static NEXT_DATABASE: AtomicU32 = AtomicU32::new(0);

/// Creates a fresh, migrated database so that tests stay independent.
pub async fn test_pool() -> Pool {
    let admin_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let name = format!(
        "test_{}_{}",
        std::process::id(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    );
    let mut admin = AsyncPgConnection::establish(&admin_url).await.unwrap();
    diesel::sql_query(format!("CREATE DATABASE {name}"))
        .execute(&mut admin)
        .await
        .unwrap();
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    let pool = db::connect(&format!("{base}/{name}")).unwrap();
    db::migrate(&pool).await.unwrap();
    pool
}

pub fn endpoints(server: &MockServer) -> Endpoints {
    Endpoints {
        sso: server.uri(),
        di_token: format!("{}/token", server.uri()),
        api: server.uri(),
    }
}

pub fn tokens() -> Tokens {
    Tokens {
        access_token: "old-access".into(),
        refresh_token: "old-refresh".into(),
        client_id: "client".into(),
    }
}
