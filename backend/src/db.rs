use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Double};
use diesel::upsert::excluded;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool;

use diesel_async::{AsyncConnection, AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde_json::Value;

use crate::garmin::Activity;
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

#[derive(QueryableByName, Debug, PartialEq)]
pub struct GridCell {
    #[diesel(sql_type = BigInt)]
    pub ix: i64,
    #[diesel(sql_type = BigInt)]
    pub iy: i64,
    #[diesel(sql_type = BigInt)]
    pub runs: i64,
}

pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// Bins points into a Web Mercator grid and counts the distinct runs per cell,
/// so that standing still at a traffic light does not make a spot look popular.
pub async fn heatmap_cells(pool: &Pool, cell_size: f64, bounds: &Bounds) -> Result<Vec<GridCell>> {
    let cells = diesel::sql_query(
        "SELECT ix, iy, count(*) AS runs FROM (
             SELECT DISTINCT floor(x / $1)::bigint AS ix, floor(y / $1)::bigint AS iy, run_id
             FROM track_points
             WHERE lon BETWEEN $2 AND $3 AND lat BETWEEN $4 AND $5
         ) AS visits
         GROUP BY ix, iy",
    )
    .bind::<Double, _>(cell_size)
    .bind::<Double, _>(bounds.west)
    .bind::<Double, _>(bounds.east)
    .bind::<Double, _>(bounds.south)
    .bind::<Double, _>(bounds.north)
    .load(&mut pool.get().await?)
    .await?;
    Ok(cells)
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
