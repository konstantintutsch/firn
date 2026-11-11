use anyhow::{Context, Result};
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool;
use diesel_async::{AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};

mod activities;
mod activity_images;
mod daily_metrics;
mod garmin_documents;
mod race_results;
mod routes;
pub mod schema;
mod sessions;
mod track_points;
mod users;

pub use activities::*;
pub use activity_images::*;
pub use daily_metrics::*;
pub use garmin_documents::*;
pub use race_results::*;
pub use routes::*;
pub use sessions::*;
pub use track_points::*;
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
