use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool;

use diesel_async::{AsyncConnection, AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde_json::Value;

use crate::garmin::Activity;
use crate::heatmap::Track;
use crate::schema::{runs, track_points};
use crate::track::Point;

pub type Pool = deadpool::Pool<AsyncPgConnection>;

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
/// Postgres allows 65535 bind parameters per statement, and a point uses 4.
const POINTS_PER_INSERT: usize = 10_000;

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

#[derive(Insertable)]
#[diesel(table_name = runs)]
struct NewRun<'a> {
    id: i64,
    name: &'a str,
    activity_type: &'a str,
    started_at: DateTime<Utc>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    summary: &'a Value,
}

/// Inserts or updates activity metadata. Never touches a run's saved track.
pub async fn upsert_runs(pool: &Pool, activities: &[Activity]) -> Result<()> {
    let rows: Vec<_> = activities
        .iter()
        .map(|activity| NewRun {
            id: activity.id,
            name: &activity.name,
            activity_type: &activity.activity_type,
            started_at: activity.started_at,
            distance_m: activity.distance_m,
            duration_s: activity.duration_s,
            summary: &activity.summary,
        })
        .collect();
    diesel::insert_into(runs::table)
        .values(&rows)
        .on_conflict(runs::id)
        .do_update()
        .set((
            runs::name.eq(excluded(runs::name)),
            runs::activity_type.eq(excluded(runs::activity_type)),
            runs::started_at.eq(excluded(runs::started_at)),
            runs::distance_m.eq(excluded(runs::distance_m)),
            runs::duration_s.eq(excluded(runs::duration_s)),
            runs::summary.eq(excluded(runs::summary)),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

pub async fn is_track_synced(pool: &Pool, id: i64) -> Result<bool> {
    let synced_at: Option<DateTime<Utc>> = runs::table
        .find(id)
        .select(runs::track_synced_at)
        .first(&mut pool.get().await?)
        .await?;
    Ok(synced_at.is_some())
}

/// Stores a track atomically and marks the run as done. `None` means no track exists.
pub async fn save_track(
    pool: &Pool,
    id: i64,
    original_zip: Option<&[u8]>,
    points: &[Point],
) -> Result<()> {
    let rows: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(seq, point)| {
            (
                track_points::run_id.eq(id),
                track_points::seq.eq(seq as i32),
                track_points::lat.eq(point.lat),
                track_points::lon.eq(point.lon),
            )
        })
        .collect();
    let mut connection = pool.get().await?;
    connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            diesel::delete(track_points::table.filter(track_points::run_id.eq(id)))
                .execute(&mut *connection)
                .await?;
            for chunk in rows.chunks(POINTS_PER_INSERT) {
                diesel::insert_into(track_points::table)
                    .values(chunk)
                    .execute(&mut *connection)
                    .await?;
            }
            diesel::update(runs::table.find(id))
                .set((
                    runs::original_zip.eq(original_zip),
                    runs::track_synced_at.eq(diesel::dsl::now),
                ))
                .execute(&mut *connection)
                .await?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Loads the Web Mercator positions of all tracks, each in recording order.
pub async fn load_tracks(pool: &Pool) -> Result<Vec<Track>> {
    let rows: Vec<(i64, Option<f64>, Option<f64>)> = track_points::table
        .select((track_points::run_id, track_points::x, track_points::y))
        .order((track_points::run_id, track_points::seq))
        .load(&mut pool.get().await?)
        .await?;

    let mut tracks = Vec::new();
    let mut current_run = None;
    for (run_id, x, y) in rows {
        // Never NULL: the database generates them from columns that cannot be.
        let (Some(x), Some(y)) = (x, y) else { continue };
        if current_run != Some(run_id) {
            current_run = Some(run_id);
            tracks.push(Track::new());
        }
        if let Some(track) = tracks.last_mut() {
            track.push([x, y]);
        }
    }
    Ok(tracks)
}

pub struct Totals {
    pub runs: i64,
    pub distance_m: f64,
}

pub async fn totals(pool: &Pool) -> Result<Totals> {
    let (count, distance): (i64, Option<f64>) = runs::table
        .select((
            diesel::dsl::count_star(),
            diesel::dsl::sum(runs::distance_m),
        ))
        .first(&mut pool.get().await?)
        .await?;
    Ok(Totals {
        runs: count,
        distance_m: distance.unwrap_or(0.0),
    })
}
