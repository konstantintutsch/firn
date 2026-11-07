use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use diesel::dsl::{count_star, exists, min, sum};
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
use crate::schema::{activities, daily_metrics, garmin_documents, sessions, track_points, users};
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
    user_id: i64,
    name: &'a str,
    activity_type: &'a str,
    started_at: DateTime<Utc>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    summary: &'a Value,
}

/// Inserts or updates activity metadata. Never touches a run's saved track, and
/// never moves an activity that another account saved already.
pub async fn upsert_activities(pool: &Pool, user_id: i64, activities: &[Activity]) -> Result<()> {
    let rows: Vec<_> = activities
        .iter()
        .map(|activity| NewActivity {
            id: activity.id,
            user_id,
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
            activities::user_id.eq(diesel::dsl::sql::<
                diesel::sql_types::Nullable<diesel::sql_types::Int8>,
            >(
                "coalesce(activities.user_id, excluded.user_id)"
            )),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// What a sync still has to download for an activity.
pub struct Missing {
    pub track: bool,
    pub details: bool,
    pub workouts: bool,
}

/// `None` if the activity belongs to another account.
pub async fn missing(pool: &Pool, user_id: i64, id: i64) -> Result<Option<Missing>> {
    let row: Option<(Option<DateTime<Utc>>, bool, bool)> = activities::table
        .find(id)
        .filter(activities::user_id.eq(user_id))
        .select((
            activities::track_synced_at,
            activities::details.is_not_null(),
            activities::workouts.is_not_null(),
        ))
        .first(&mut pool.get().await?)
        .await
        .optional()?;
    Ok(row.map(|(synced_at, has_details, has_workouts)| Missing {
        track: synced_at.is_none(),
        details: !has_details,
        workouts: !has_workouts,
    }))
}

/// `details` is an empty object if Garmin had none, so that no sync asks again.
pub async fn save_details(pool: &Pool, id: i64, details: &Value) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set(activities::details.eq(details))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// `workouts` is an empty array if Garmin had none, so that no sync asks again.
pub async fn save_workouts(pool: &Pool, id: i64, workouts: &Value) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set(activities::workouts.eq(workouts))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
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

/// Loads the Web Mercator positions of an account's tracks, each in recording order.
pub async fn load_tracks(pool: &Pool, user_id: i64) -> Result<Vec<ActivityTrack>> {
    let rows: Vec<(i64, Option<f64>, Option<f64>)> = track_points::table
        .inner_join(activities::table)
        .filter(activities::user_id.eq(user_id))
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
    /// The account whose activities count.
    pub user_id: i64,
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
    /// Also counts activities without GPS, which have no place on a map but do in statistics.
    pub without_gps: bool,
}

impl Filter {
    fn apply<'a, ST: 'a>(
        &self,
        mut query: activities::BoxedQuery<'a, Pg, ST>,
    ) -> activities::BoxedQuery<'a, Pg, ST> {
        query = query.filter(activities::user_id.eq(self.user_id));
        if !self.without_gps {
            query = query.filter(exists(
                track_points::table.filter(track_points::activity_id.eq(activities::id)),
            ));
        }
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

/// The newest activities that match, after skipping the `offset` newest.
pub async fn search_activities(
    pool: &Pool,
    filter: &Filter,
    limit: i64,
    offset: i64,
) -> Result<Vec<ActivitySummary>> {
    Ok(filter
        .apply(
            activities::table
                .select(ActivitySummary::as_select())
                .into_boxed(),
        )
        .order((activities::started_at.desc(), activities::id.desc()))
        .limit(limit)
        .offset(offset)
        .load(&mut pool.get().await?)
        .await?)
}

/// One activity of an account, with Garmin's summary and the original file.
pub async fn activity_with_file(
    pool: &Pool,
    user_id: i64,
    id: i64,
) -> Result<Option<(ActivitySummary, Value, Option<Vec<u8>>)>> {
    Ok(activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(activities::id.eq(id))
        .select((
            ActivitySummary::as_select(),
            activities::summary,
            activities::original_zip,
        ))
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// Each activity type that has a track, with how many activities have it, most first.
pub async fn activity_types(
    pool: &Pool,
    user_id: i64,
    without_gps: bool,
) -> Result<Vec<(String, i64)>> {
    let mut query = activities::table
        .filter(activities::user_id.eq(user_id))
        .group_by(activities::activity_type)
        .select((activities::activity_type, count_star()))
        .order((count_star().desc(), activities::activity_type))
        .into_boxed();
    if !without_gps {
        query = query.filter(exists(
            track_points::table.filter(track_points::activity_id.eq(activities::id)),
        ));
    }
    Ok(query.load(&mut pool.get().await?).await?)
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

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = users)]
pub struct User {
    pub id: i64,
    pub email: Option<String>,
    pub name: Option<String>,
    pub garmin_api_key: Option<Vec<u8>>,
}

/// Finds or creates the account of an OpenID Connect identity. The first account
/// claims the activities that were saved before accounts existed.
pub async fn sign_in_user(
    pool: &Pool,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<(User, bool)> {
    let mut connection = pool.get().await?;
    let result = connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            // Serializes first sign-ins, so that only one account can be first.
            diesel::sql_query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
                .execute(&mut *connection)
                .await?;
            let existing: Option<User> = users::table
                .filter(users::oidc_issuer.eq(issuer))
                .filter(users::oidc_subject.eq(subject))
                .select(User::as_select())
                .first(&mut *connection)
                .await
                .optional()?;
            if let Some(user) = existing {
                let user = diesel::update(users::table.find(user.id))
                    .set((users::email.eq(email), users::name.eq(name)))
                    .returning(User::as_returning())
                    .get_result(&mut *connection)
                    .await?;
                return Ok((user, false));
            }
            let is_first = users::table
                .count()
                .get_result::<i64>(&mut *connection)
                .await?
                == 0;
            let user = diesel::insert_into(users::table)
                .values((
                    users::oidc_issuer.eq(issuer),
                    users::oidc_subject.eq(subject),
                    users::email.eq(email),
                    users::name.eq(name),
                ))
                .returning(User::as_returning())
                .get_result(&mut *connection)
                .await?;
            if is_first {
                diesel::update(activities::table.filter(activities::user_id.is_null()))
                    .set(activities::user_id.eq(user.id))
                    .execute(&mut *connection)
                    .await?;
            }
            Ok((user, is_first))
        })
        .await?;
    Ok(result)
}

pub async fn set_garmin_key(pool: &Pool, user_id: i64, sealed: Option<&[u8]>) -> Result<()> {
    diesel::update(users::table.find(user_id))
        .set(users::garmin_api_key.eq(sealed))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

pub async fn create_session(
    pool: &Pool,
    user_id: i64,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<()> {
    diesel::insert_into(sessions::table)
        .values((
            sessions::token_hash.eq(token_hash),
            sessions::user_id.eq(user_id),
            sessions::expires_at.eq(expires_at),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// The account of a session that has not expired.
pub async fn session_user(pool: &Pool, token_hash: &[u8]) -> Result<Option<User>> {
    Ok(sessions::table
        .inner_join(users::table)
        .filter(sessions::token_hash.eq(token_hash))
        .filter(sessions::expires_at.gt(diesel::dsl::now))
        .select(User::as_select())
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// Also removes expired sessions of all accounts, which nothing else cleans up.
pub async fn delete_session(pool: &Pool, token_hash: &[u8]) -> Result<()> {
    diesel::delete(
        sessions::table.filter(
            sessions::token_hash
                .eq(token_hash)
                .or(sessions::expires_at.le(diesel::dsl::now)),
        ),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

/// The periods of a kind that are saved already.
pub async fn document_periods(pool: &Pool, user_id: i64, kind: &str) -> Result<HashSet<String>> {
    let periods: Vec<String> = garmin_documents::table
        .filter(garmin_documents::user_id.eq(user_id))
        .filter(garmin_documents::kind.eq(kind))
        .select(garmin_documents::period)
        .load(&mut pool.get().await?)
        .await?;
    Ok(periods.into_iter().collect())
}

/// Saves a response and the daily values read from it, replacing earlier ones.
pub async fn save_document(
    pool: &Pool,
    user_id: i64,
    kind: &str,
    period: &str,
    data: Option<&Value>,
    metrics: &[(NaiveDate, &str, f64)],
) -> Result<()> {
    let rows: Vec<_> = metrics
        .iter()
        .map(|(day, metric, value)| {
            (
                daily_metrics::user_id.eq(user_id),
                daily_metrics::day.eq(*day),
                daily_metrics::metric.eq(*metric),
                daily_metrics::value.eq(*value),
            )
        })
        .collect();
    let mut connection = pool.get().await?;
    connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            diesel::insert_into(garmin_documents::table)
                .values((
                    garmin_documents::user_id.eq(user_id),
                    garmin_documents::kind.eq(kind),
                    garmin_documents::period.eq(period),
                    garmin_documents::data.eq(data),
                ))
                .on_conflict((
                    garmin_documents::user_id,
                    garmin_documents::kind,
                    garmin_documents::period,
                ))
                .do_update()
                .set((
                    garmin_documents::data.eq(excluded(garmin_documents::data)),
                    garmin_documents::fetched_at.eq(diesel::dsl::now),
                ))
                .execute(&mut *connection)
                .await?;
            if !rows.is_empty() {
                diesel::insert_into(daily_metrics::table)
                    .values(&rows)
                    .on_conflict((
                        daily_metrics::user_id,
                        daily_metrics::metric,
                        daily_metrics::day,
                    ))
                    .do_update()
                    .set(daily_metrics::value.eq(excluded(daily_metrics::value)))
                    .execute(&mut *connection)
                    .await?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

/// The first day of saved activities in local time, where the health data starts.
pub async fn first_activity_day(pool: &Pool, user_id: i64) -> Result<Option<NaiveDate>> {
    let first: Option<String> = activities::table
        .filter(activities::user_id.eq(user_id))
        .select(min(activities::started_local))
        .first(&mut pool.get().await?)
        .await?;
    Ok(first.and_then(|text| NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()))
}

/// Daily values from `from` to `to`, sorted by metric and day.
pub async fn daily_metrics(
    pool: &Pool,
    user_id: i64,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<(String, NaiveDate, f64)>> {
    let mut query = daily_metrics::table
        .filter(daily_metrics::user_id.eq(user_id))
        .select((
            daily_metrics::metric,
            daily_metrics::day,
            daily_metrics::value,
        ))
        .order((daily_metrics::metric, daily_metrics::day))
        .into_boxed();
    if let Some(from) = from {
        query = query.filter(daily_metrics::day.ge(from));
    }
    if let Some(to) = to {
        query = query.filter(daily_metrics::day.le(to));
    }
    Ok(query.load(&mut pool.get().await?).await?)
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = activities)]
pub struct ActivityRecord {
    pub id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_local: Option<String>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub summary: Value,
}

/// The matching activities, oldest first.
pub async fn activity_records(pool: &Pool, filter: &Filter) -> Result<Vec<ActivityRecord>> {
    Ok(filter
        .apply(
            activities::table
                .select(ActivityRecord::as_select())
                .order((activities::started_at, activities::id))
                .into_boxed(),
        )
        .load(&mut pool.get().await?)
        .await?)
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
