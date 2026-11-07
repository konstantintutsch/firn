use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use diesel::dsl::{count_star, exists, sum};
use diesel::pg::Pg;
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool;

use diesel_async::{AsyncConnection, AsyncMigrationHarness, AsyncPgConnection, RunQueryDsl};
use diesel_migrations::{EmbeddedMigrations, MigrationHarness, embed_migrations};
use serde_json::Value;

use crate::garmin::Activity;
use crate::heatmap::Track;
use crate::schema::{activities, track_points};
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
#[diesel(table_name = activities)]
struct NewActivity<'a> {
    id: i64,
    name: &'a str,
    activity_type: &'a str,
    started_at: DateTime<Utc>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    summary: &'a Value,
}

/// Inserts or updates activity metadata. Never touches a run's saved track.
pub async fn upsert_activities(pool: &Pool, activities: &[Activity]) -> Result<()> {
    let rows: Vec<_> = activities
        .iter()
        .map(|activity| NewActivity {
            id: activity.id,
            name: &activity.name,
            activity_type: &activity.activity_type,
            started_at: activity.started_at,
            distance_m: activity.distance_m,
            duration_s: activity.duration_s,
            summary: &activity.summary,
        })
        .collect();
    diesel::insert_into(activities::table)
        .values(&rows)
        .on_conflict(activities::id)
        .do_update()
        .set((
            activities::name.eq(excluded(activities::name)),
            activities::activity_type.eq(excluded(activities::activity_type)),
            activities::started_at.eq(excluded(activities::started_at)),
            activities::distance_m.eq(excluded(activities::distance_m)),
            activities::duration_s.eq(excluded(activities::duration_s)),
            activities::summary.eq(excluded(activities::summary)),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

pub async fn is_track_synced(pool: &Pool, id: i64) -> Result<bool> {
    let synced_at: Option<DateTime<Utc>> = activities::table
        .find(id)
        .select(activities::track_synced_at)
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
                track_points::activity_id.eq(id),
                track_points::seq.eq(seq as i32),
                track_points::lat.eq(point.lat),
                track_points::lon.eq(point.lon),
            )
        })
        .collect();
    let mut connection = pool.get().await?;
    connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            diesel::delete(track_points::table.filter(track_points::activity_id.eq(id)))
                .execute(&mut *connection)
                .await?;
            for chunk in rows.chunks(POINTS_PER_INSERT) {
                diesel::insert_into(track_points::table)
                    .values(chunk)
                    .execute(&mut *connection)
                    .await?;
            }
            diesel::update(activities::table.find(id))
                .set((
                    activities::original_zip.eq(original_zip),
                    activities::track_synced_at.eq(diesel::dsl::now),
                ))
                .execute(&mut *connection)
                .await?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// Loads the Web Mercator positions of all tracks, each in recording order.
pub async fn load_tracks(pool: &Pool) -> Result<Vec<ActivityTrack>> {
    let rows: Vec<(i64, Option<f64>, Option<f64>)> = track_points::table
        .select((track_points::activity_id, track_points::x, track_points::y))
        .order((track_points::activity_id, track_points::seq))
        .load(&mut pool.get().await?)
        .await?;

    let mut tracks: Vec<ActivityTrack> = Vec::new();
    for (activity_id, x, y) in rows {
        // Never NULL: the database generates them from columns that cannot be.
        let (Some(x), Some(y)) = (x, y) else { continue };
        match tracks.last_mut() {
            Some(last) if last.id == activity_id => last.track.push([x, y]),
            _ => tracks.push(ActivityTrack {
                id: activity_id,
                track: vec![[x, y]],
            }),
        }
    }
    Ok(tracks)
}

pub struct ActivityTrack {
    pub id: i64,
    pub track: Track,
}

/// Limits which activities count. All set fields must match.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub id: Option<i64>,
    pub query: Option<String>,
    pub activity_types: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub min_distance_m: Option<f64>,
    pub max_distance_m: Option<f64>,
    pub min_duration_s: Option<f64>,
    pub max_duration_s: Option<f64>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

impl Filter {
    /// Older syncs saved activities without GPS, which have no place on a map.
    fn apply<'a, ST: 'a>(
        &self,
        mut query: activities::BoxedQuery<'a, Pg, ST>,
    ) -> activities::BoxedQuery<'a, Pg, ST> {
        query = query.filter(exists(
            track_points::table.filter(track_points::activity_id.eq(activities::id)),
        ));
        if let Some(id) = self.id {
            query = query.filter(activities::id.eq(id));
        }
        for term in search_terms(self.query.as_deref().unwrap_or_default()) {
            let text = activities::search_text.ilike(contains(&term.text));
            query = match term.amount {
                Some(Amount::Meters(meters)) => {
                    let (low, high) = around(meters);
                    query.filter(text.or(activities::distance_m.between(low, high)))
                }
                Some(Amount::Seconds(seconds)) => {
                    let (low, high) = around(seconds);
                    query.filter(text.or(activities::duration_s.between(low, high)))
                }
                None => query.filter(text),
            };
        }
        if !self.activity_types.is_empty() {
            query = query.filter(activities::activity_type.eq_any(self.activity_types.clone()));
        }
        if let Some(title) = &self.title {
            query = query.filter(activities::name.ilike(contains(title)));
        }
        if let Some(description) = &self.description {
            query = query.filter(activities::description.ilike(contains(description)));
        }
        if let Some(location) = &self.location {
            query = query.filter(activities::location.ilike(contains(location)));
        }
        if let Some(min) = self.min_distance_m {
            query = query.filter(activities::distance_m.ge(min));
        }
        if let Some(max) = self.max_distance_m {
            query = query.filter(activities::distance_m.le(max));
        }
        if let Some(min) = self.min_duration_s {
            query = query.filter(activities::duration_s.ge(min));
        }
        if let Some(max) = self.max_duration_s {
            query = query.filter(activities::duration_s.le(max));
        }
        if let Some(from) = self.from {
            query = query.filter(activities::started_local.ge(from.to_string()));
        }
        // A prefix sorts before all times on its day, so this needs the next day.
        if let Some(next_day) = self.to.and_then(|to| to.succ_opt()) {
            query = query.filter(activities::started_local.lt(next_day.to_string()));
        }
        query
    }
}

/// How far a searched length may be off, since no 10 km run measures exactly 10 km.
const AMOUNT_TOLERANCE: f64 = 0.05;

fn around(amount: f64) -> (f64, f64) {
    (
        amount * (1.0 - AMOUNT_TOLERANCE),
        amount * (1.0 + AMOUNT_TOLERANCE),
    )
}

#[derive(Debug, PartialEq)]
enum Amount {
    Meters(f64),
    Seconds(f64),
}

#[derive(Debug, PartialEq)]
struct SearchTerm {
    text: String,
    amount: Option<Amount>,
}

/// Splits a search into words that must all match. A word like `10km` or `45min`, or
/// `10 km` with a space, also matches activities of about that length.
fn search_terms(search: &str) -> Vec<SearchTerm> {
    let mut words = search.split_whitespace().peekable();
    let mut terms = Vec::new();
    while let Some(word) = words.next() {
        let spaced_amount = words
            .peek()
            .and_then(|unit| parse_amount(&format!("{word}{unit}")));
        if word.parse::<f64>().is_ok()
            && let Some(amount) = spaced_amount
            && let Some(unit) = words.next()
        {
            terms.push(SearchTerm {
                text: format!("{word} {unit}"),
                amount: Some(amount),
            });
        } else {
            terms.push(SearchTerm {
                text: word.to_owned(),
                amount: parse_amount(word),
            });
        }
    }
    terms
}

fn parse_amount(word: &str) -> Option<Amount> {
    let word = word.to_lowercase();
    let split = word.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = word.split_at(split);
    let number: f64 = number.parse().ok()?;
    match unit {
        "km" | "k" => Some(Amount::Meters(number * 1000.0)),
        "m" => Some(Amount::Meters(number)),
        "mi" => Some(Amount::Meters(number * 1609.344)),
        "min" => Some(Amount::Seconds(number * 60.0)),
        "h" => Some(Amount::Seconds(number * 3600.0)),
        _ => None,
    }
}

/// A LIKE pattern that finds the text anywhere, with its wildcards taken literally.
fn contains(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The activities that match, for the heatmap to pick from its tracks.
pub async fn matching_ids(pool: &Pool, filter: &Filter) -> Result<HashSet<i64>> {
    let ids: Vec<i64> = filter
        .apply(activities::table.select(activities::id).into_boxed())
        .load(&mut pool.get().await?)
        .await?;
    Ok(ids.into_iter().collect())
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = activities)]
pub struct ActivitySummary {
    pub id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub description: Option<String>,
    pub location: Option<String>,
}

/// The newest activities that match.
pub async fn search_activities(
    pool: &Pool,
    filter: &Filter,
    limit: i64,
) -> Result<Vec<ActivitySummary>> {
    Ok(filter
        .apply(
            activities::table
                .select(ActivitySummary::as_select())
                .into_boxed(),
        )
        .order((activities::started_at.desc(), activities::id.desc()))
        .limit(limit)
        .load(&mut pool.get().await?)
        .await?)
}

/// Each activity type that has a track, with how many activities have it, most first.
pub async fn activity_types(pool: &Pool) -> Result<Vec<(String, i64)>> {
    Ok(activities::table
        .filter(exists(
            track_points::table.filter(track_points::activity_id.eq(activities::id)),
        ))
        .group_by(activities::activity_type)
        .select((activities::activity_type, count_star()))
        .order((count_star().desc(), activities::activity_type))
        .load(&mut pool.get().await?)
        .await?)
}

pub struct Totals {
    pub activities: i64,
    pub distance_m: f64,
}

pub async fn totals(pool: &Pool, filter: &Filter) -> Result<Totals> {
    let (count, distance): (i64, Option<f64>) = filter
        .apply(
            activities::table
                .select((count_star(), sum(activities::distance_m)))
                .into_boxed(),
        )
        .first(&mut pool.get().await?)
        .await?;
    Ok(Totals {
        activities: count,
        distance_m: distance.unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, amount: Option<Amount>) -> SearchTerm {
        SearchTerm {
            text: text.to_owned(),
            amount,
        }
    }

    #[test]
    fn search_finds_lengths_with_and_without_a_space() {
        assert_eq!(
            search_terms("park 10km 5 k 45min 2 parks"),
            [
                term("park", None),
                term("10km", Some(Amount::Meters(10_000.0))),
                term("5 k", Some(Amount::Meters(5_000.0))),
                term("45min", Some(Amount::Seconds(2_700.0))),
                term("2", None),
                term("parks", None),
            ]
        );
    }

    #[test]
    fn words_that_only_look_like_lengths_stay_text() {
        assert_eq!(parse_amount("k"), None);
        assert_eq!(parse_amount("10"), None);
        assert_eq!(parse_amount("1.2.3km"), None);
        assert_eq!(parse_amount("5kg"), None);
    }

    #[test]
    fn patterns_take_wildcards_literally() {
        assert_eq!(contains(r"50%_a\b"), r"%50\%\_a\\b%");
    }
}
