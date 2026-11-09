use anyhow::{Context, Result};
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool;
use diesel_async::{AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

mod activities;
mod backfill;
mod documents;
mod export;
mod filter;
mod images;
mod metrics;
mod race_results;
mod routes;
mod search;
mod sessions;
mod tracks;
mod training_blocks;
mod users;

pub use activities::*;
pub use backfill::*;
pub use documents::*;
pub use export::*;
pub use filter::*;
pub use images::*;
pub use metrics::*;
pub use race_results::*;
pub use routes::*;
pub use search::*;
pub use sessions::*;
pub use tracks::*;
pub use training_blocks::*;
pub use users::*;

pub type Pool = deadpool::Pool<AsyncPgConnection>;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");

pub fn connect(database_url: &str) -> Result<Pool> {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
    Pool::builder(manager)
        .max_size(8)
        .build()
        .context("could not create database pool")
}

pub async fn migrate(pool: &Pool) -> Result<()> {
    let connection = pool.get().await.context("could not reach database")?;
    tokio::task::spawn_blocking(move || {
        AsyncMigrationHarness::from(connection)
            .run_pending_migrations(MIGRATIONS)
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("migration failed: {error}"))
    })
    .await?
}

pub async fn ping(pool: &Pool) -> Result<()> {
    diesel::sql_query("SELECT 1")
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}
