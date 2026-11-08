use std::collections::{HashMap, HashSet};

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
use serde_json::{Value, json};
use uuid::Uuid;

use crate::files::FileStore;
use crate::garmin::Activity;
use crate::heatmap::{self, Track, Viewport};
use crate::schema::{
    activities, activity_images, daily_metrics, garmin_documents, race_results, routes, sessions,
    track_points, users,
};
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
    /// `None` gives the activity a new random ID, unless it is saved already.
    id: Option<Uuid>,
    garmin_id: i64,
    user_id: i64,
    name: &'a str,
    activity_type: &'a str,
    started_at: DateTime<Utc>,
    distance_m: Option<f64>,
    duration_s: Option<f64>,
    summary: &'a Value,
}

/// Inserts or updates activity metadata. Never touches a run's saved track, and
/// never changes or moves an activity that another account saved already.
pub async fn upsert_activities(pool: &Pool, user_id: i64, activities: &[Activity]) -> Result<()> {
    let activities: Vec<_> = activities.iter().map(|activity| (activity, None)).collect();
    upsert_activities_with_ids(pool, user_id, &activities).await
}

/// Like `upsert_activities`, and a new activity takes the ID that goes with it, if any.
pub async fn upsert_activities_with_ids(
    pool: &Pool,
    user_id: i64,
    activities: &[(&Activity, Option<Uuid>)],
) -> Result<()> {
    let rows: Vec<_> = activities
        .iter()
        .map(|(activity, id)| NewActivity {
            id: *id,
            garmin_id: activity.id,
            user_id,
            name: &activity.name,
            activity_type: &activity.activity_type,
            started_at: activity.started_at,
            distance_m: activity.distance_m,
            duration_s: activity.duration_s,
            summary: &activity.summary,
        })
        .collect();
    let upsert = diesel::insert_into(activities::table)
        .values(&rows)
        .on_conflict(activities::garmin_id)
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
        ));
    // Only ON CONFLICT … WHERE has this `filter`; QueryDsl's does not apply here.
    diesel::query_dsl::methods::FilterDsl::filter(
        upsert,
        activities::user_id
            .is_null()
            .or(activities::user_id.eq(user_id)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

/// What a sync still has to download for an activity.
pub struct Missing {
    pub track: bool,
    pub details: bool,
    pub workouts: bool,
    pub images: bool,
}

/// `None` if the activity belongs to another account.
/// The summary of one of the account's activities, as Garmin's list sent it.
pub async fn summary(pool: &Pool, user_id: i64, id: Uuid) -> Result<Option<Value>> {
    Ok(activities::table
        .find(id)
        .filter(activities::user_id.eq(user_id))
        .select(activities::summary)
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// When the track was saved, and whether details, workouts and images were.
type MissingRow = (Option<DateTime<Utc>>, bool, bool, bool);

impl Missing {
    fn from_row((synced_at, has_details, has_workouts, has_images): MissingRow) -> Self {
        Self {
            track: synced_at.is_none(),
            details: !has_details,
            workouts: !has_workouts,
            images: !has_images,
        }
    }
}

/// The ID and what is missing for each of the account's activities with these
/// Garmin IDs, in one query, by Garmin ID. Activities of other accounts are left out.
pub async fn missing_many(
    pool: &Pool,
    user_id: i64,
    garmin_ids: &[i64],
) -> Result<HashMap<i64, (Uuid, Missing)>> {
    let rows: Vec<(i64, Uuid, MissingRow)> = activities::table
        .filter(activities::garmin_id.eq_any(garmin_ids))
        .filter(activities::user_id.eq(user_id))
        .select((
            activities::garmin_id,
            activities::id,
            (
                activities::track_synced_at,
                activities::details.is_not_null(),
                activities::workouts.is_not_null(),
                activities::images_synced_at.is_not_null(),
            ),
        ))
        .load(&mut pool.get().await?)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(garmin_id, id, row)| (garmin_id, (id, Missing::from_row(row))))
        .collect())
}

pub async fn missing(pool: &Pool, user_id: i64, id: Uuid) -> Result<Option<Missing>> {
    let row: Option<MissingRow> = activities::table
        .find(id)
        .filter(activities::user_id.eq(user_id))
        .select((
            activities::track_synced_at,
            activities::details.is_not_null(),
            activities::workouts.is_not_null(),
            activities::images_synced_at.is_not_null(),
        ))
        .first(&mut pool.get().await?)
        .await
        .optional()?;
    Ok(row.map(Missing::from_row))
}

/// Marks the images of activities as done whose saved details list none, so that
/// a sync does not wait for each of them.
pub async fn skip_activities_without_images(pool: &Pool, user_id: i64) -> Result<()> {
    diesel::update(
        activities::table
            .filter(activities::user_id.eq(user_id))
            .filter(activities::images_synced_at.is_null())
            .filter(activities::details.is_not_null())
            .filter(diesel::dsl::sql::<diesel::sql_types::Bool>(
                "coalesce(details #> '{metadataDTO,activityImages}', '[]') = '[]'",
            )),
    )
    .set(activities::images_synced_at.eq(diesel::dsl::now))
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

pub async fn details(pool: &Pool, id: Uuid) -> Result<Option<Value>> {
    Ok(activities::table
        .find(id)
        .select(activities::details)
        .first::<Option<Value>>(&mut pool.get().await?)
        .await
        .optional()?
        .flatten())
}

/// An image whose data is in the file store.
pub struct Image {
    pub id: String,
    pub content_type: String,
}

/// Replaces the images of an activity atomically and marks them as done.
pub async fn save_images(pool: &Pool, id: Uuid, images: &[Image]) -> Result<()> {
    let rows: Vec<_> = images
        .iter()
        .enumerate()
        .map(|(seq, image)| {
            (
                activity_images::activity_id.eq(id),
                activity_images::image_id.eq(&image.id),
                activity_images::seq.eq(seq as i32),
                activity_images::content_type.eq(&image.content_type),
            )
        })
        .collect();
    let mut connection = pool.get().await?;
    connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            diesel::delete(activity_images::table.filter(activity_images::activity_id.eq(id)))
                .execute(&mut *connection)
                .await?;
            if !rows.is_empty() {
                diesel::insert_into(activity_images::table)
                    .values(&rows)
                    .on_conflict_do_nothing()
                    .execute(&mut *connection)
                    .await?;
            }
            diesel::update(activities::table.find(id))
                .set(activities::images_synced_at.eq(diesel::dsl::now))
                .execute(&mut *connection)
                .await?;
            Ok(())
        })
        .await?;
    Ok(())
}

/// The IDs of an activity's images, in Garmin's order.
pub async fn image_ids(pool: &Pool, id: Uuid) -> Result<Vec<String>> {
    Ok(activity_images::table
        .filter(activity_images::activity_id.eq(id))
        .order(activity_images::seq)
        .select(activity_images::image_id)
        .load(&mut pool.get().await?)
        .await?)
}

/// The media type of one image of an account's activity, if it has that image.
pub async fn image_type(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
    image_id: &str,
) -> Result<Option<String>> {
    Ok(activity_images::table
        .inner_join(activities::table)
        .filter(activities::user_id.eq(user_id))
        .filter(activity_images::activity_id.eq(id))
        .filter(activity_images::image_id.eq(image_id))
        .select(activity_images::content_type)
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// `details` is an empty object if Garmin had none, so that no sync asks again.
pub async fn save_details(pool: &Pool, id: Uuid, details: &Value) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set(activities::details.eq(details))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// `workouts` is an empty array if Garmin had none, so that no sync asks again.
pub async fn save_workouts(pool: &Pool, id: Uuid, workouts: &Value) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set(activities::workouts.eq(workouts))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Stores a track atomically and marks the run as done. `None` means no track exists.
/// Replaces the track of an activity. Its original file goes to the file store.
pub async fn save_track(pool: &Pool, id: Uuid, points: &[Point]) -> Result<()> {
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
    let preview = (!points.is_empty()).then(|| json!(crate::track::preview(points)));
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
                    activities::track_synced_at.eq(diesel::dsl::now),
                    activities::preview.eq(preview),
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
    let rows: Vec<(Uuid, Option<f64>, Option<f64>)> = track_points::table
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
                bounds: Viewport {
                    min: [x, y],
                    max: [x, y],
                },
            }),
        }
    }
    for activity in &mut tracks {
        activity.bounds = heatmap::bounds(&activity.track).expect("a track has a point");
    }
    Ok(tracks)
}

pub struct ActivityTrack {
    pub id: Uuid,
    pub track: Track,
    /// Around the whole track, so that the heatmap can skip tracks outside the viewport.
    pub bounds: Viewport,
}

/// Limits which activities count. All set fields must match.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// The account whose activities count.
    pub user_id: i64,
    pub id: Option<Uuid>,
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
pub async fn matching_ids(pool: &Pool, filter: &Filter) -> Result<HashSet<Uuid>> {
    let ids: Vec<Uuid> = filter
        .apply(activities::table.select(activities::id).into_boxed())
        .load(&mut pool.get().await?)
        .await?;
    Ok(ids.into_iter().collect())
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = activities)]
pub struct ActivitySummary {
    pub id: Uuid,
    pub garmin_id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub description: Option<String>,
    pub location: Option<String>,
    /// About 64 points along the track as `[latitude, longitude]`.
    pub preview: Option<Value>,
    /// Garmin's weather at the start, if it listed one.
    pub weather: Option<Value>,
    /// The watch and sensors that recorded the activity.
    pub devices: Option<Value>,
    /// What the activity trained most, like `TEMPO`, as Garmin names it.
    pub training_effect_label: Option<String>,
    /// How much the activity improved aerobic fitness, from 0 to 5.
    pub aerobic_training_effect: Option<f64>,
}

/// The newest activities that match, after skipping the `offset` newest.
#[derive(Debug, Clone, Copy)]
pub enum Order {
    Newest,
    Oldest,
    /// The longest distance first.
    Distance,
    /// The longest time first.
    Duration,
}

pub async fn search_activities(
    pool: &Pool,
    filter: &Filter,
    order: Order,
    limit: i64,
    offset: i64,
) -> Result<Vec<ActivitySummary>> {
    let query = filter.apply(
        activities::table
            .select(ActivitySummary::as_select())
            .into_boxed(),
    );
    // The ID keeps pages apart where the values are the same.
    let query = match order {
        Order::Newest => query.order((activities::started_at.desc(), activities::id.desc())),
        Order::Oldest => query.order((activities::started_at.asc(), activities::id.asc())),
        Order::Distance => query.order((
            activities::distance_m.desc().nulls_last(),
            activities::id.desc(),
        )),
        Order::Duration => query.order((
            activities::duration_s.desc().nulls_last(),
            activities::id.desc(),
        )),
    };
    Ok(query
        .limit(limit)
        .offset(offset)
        .load(&mut pool.get().await?)
        .await?)
}

/// One activity of an account, with Garmin's summary and the original file.
pub async fn activity_with_summary(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
) -> Result<Option<(ActivitySummary, Value)>> {
    Ok(activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(activities::id.eq(id))
        .select((ActivitySummary::as_select(), activities::summary))
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

/// The status of the account's last finished sync.
pub async fn last_sync(pool: &Pool, user_id: i64) -> Result<Option<Value>> {
    Ok(users::table
        .find(user_id)
        .select(users::last_sync)
        .first(&mut pool.get().await?)
        .await?)
}

pub async fn save_last_sync(pool: &Pool, user_id: i64, status: &Value) -> Result<()> {
    diesel::update(users::table.find(user_id))
        .set(users::last_sync.eq(status))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Deletes an account. Its activities, health data and sessions go with it.
pub async fn delete_user(pool: &Pool, user_id: i64) -> Result<()> {
    diesel::delete(users::table.find(user_id))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// The accounts that have a Garmin key, for the automatic sync.
pub async fn users_with_keys(pool: &Pool) -> Result<Vec<User>> {
    Ok(users::table
        .filter(users::garmin_api_key.is_not_null())
        .select(User::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

pub async fn set_garmin_key(pool: &Pool, user_id: i64, sealed: Option<&[u8]>) -> Result<()> {
    diesel::update(users::table.find(user_id))
        .set(users::garmin_api_key.eq(sealed))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Also removes expired sessions of all accounts, so that they do not pile up.
pub async fn create_session(
    pool: &Pool,
    user_id: i64,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let mut connection = pool.get().await?;
    diesel::delete(sessions::table.filter(sessions::expires_at.le(diesel::dsl::now)))
        .execute(&mut connection)
        .await?;
    diesel::insert_into(sessions::table)
        .values((
            sessions::token_hash.eq(token_hash),
            sessions::user_id.eq(user_id),
            sessions::expires_at.eq(expires_at),
        ))
        .execute(&mut connection)
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

/// Also removes expired sessions of all accounts.
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
    pub id: Uuid,
    pub name: String,
    pub activity_type: String,
    pub started_local: Option<String>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    pub summary: Value,
    pub vo2_max: Option<f64>,
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

/// The official result of a race, entered by hand.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Queryable,
    Selectable,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[diesel(table_name = race_results)]
pub struct RaceResult {
    /// The race's distance, like 21097.5 for a half marathon.
    pub distance_m: f64,
    /// The official finish time.
    pub time_s: f64,
    /// The name of the race, if it differs from the activity's.
    pub event: Option<String>,
    /// Where the official results are published.
    pub results_url: Option<String>,
    /// The finish time that was aimed for.
    #[serde(default)]
    pub target_time_s: Option<f64>,
}

/// The race results of activities, by activity.
pub async fn race_results(pool: &Pool, ids: &[Uuid]) -> Result<HashMap<Uuid, RaceResult>> {
    let rows: Vec<(Uuid, RaceResult)> = race_results::table
        .filter(race_results::activity_id.eq_any(ids))
        .select((race_results::activity_id, RaceResult::as_select()))
        .load(&mut pool.get().await?)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Saves the result of one of the account's activities. False if it has no such activity.
pub async fn save_race_result(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
    result: &RaceResult,
) -> Result<bool> {
    let mut connection = pool.get().await?;
    let owned: bool = diesel::select(exists(
        activities::table
            .find(id)
            .filter(activities::user_id.eq(user_id)),
    ))
    .get_result(&mut connection)
    .await?;
    if !owned {
        return Ok(false);
    }
    let row = (
        race_results::activity_id.eq(id),
        race_results::distance_m.eq(result.distance_m),
        race_results::time_s.eq(result.time_s),
        race_results::event.eq(&result.event),
        race_results::results_url.eq(&result.results_url),
        race_results::target_time_s.eq(result.target_time_s),
    );
    diesel::insert_into(race_results::table)
        .values(row)
        .on_conflict(race_results::activity_id)
        .do_update()
        .set(row)
        .execute(&mut connection)
        .await?;
    Ok(true)
}

/// Removes the result of one of the account's activities.
pub async fn delete_race_result(pool: &Pool, user_id: i64, id: Uuid) -> Result<()> {
    diesel::delete(
        race_results::table.filter(
            race_results::activity_id.eq_any(
                activities::table
                    .filter(activities::id.eq(id))
                    .filter(activities::user_id.eq(user_id))
                    .select(activities::id),
            ),
        ),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

pub async fn ping(pool: &Pool) -> Result<()> {
    diesel::sql_query("SELECT 1")
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// The data of one of the account's Garmin documents, like its heart rate zones.
pub async fn document(
    pool: &Pool,
    user_id: i64,
    kind: &str,
    period: &str,
) -> Result<Option<Value>> {
    Ok(garmin_documents::table
        .find((user_id, kind, period))
        .select(garmin_documents::data)
        .first::<Option<Value>>(&mut pool.get().await?)
        .await
        .optional()?
        .flatten())
}

/// The account's activities that started just before and just after one, by
/// start and then ID, so that activities with the same start still take turns.
pub async fn neighbors(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
    started_at: DateTime<Utc>,
) -> Result<(Option<Uuid>, Option<Uuid>)> {
    let mut connection = pool.get().await?;
    let previous = activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(
            activities::started_at
                .lt(started_at)
                .or(activities::started_at
                    .eq(started_at)
                    .and(activities::id.lt(id))),
        )
        .order((activities::started_at.desc(), activities::id.desc()))
        .select(activities::id)
        .first(&mut connection)
        .await
        .optional()?;
    let next = activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(
            activities::started_at
                .gt(started_at)
                .or(activities::started_at
                    .eq(started_at)
                    .and(activities::id.gt(id))),
        )
        .order((activities::started_at, activities::id))
        .select(activities::id)
        .first(&mut connection)
        .await
        .optional()?;
    Ok((previous, next))
}

/// The IDs of all of the account's activities, oldest first.
pub async fn activity_ids(pool: &Pool, user_id: i64) -> Result<Vec<Uuid>> {
    Ok(activities::table
        .filter(activities::user_id.eq(user_id))
        .order((activities::started_at, activities::id))
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?)
}

/// Everything about activities that an export holds, apart from files.
pub struct ActivityExport {
    pub id: Uuid,
    pub summary: Value,
    pub details: Option<Value>,
    pub workouts: Option<Value>,
    /// Image IDs with their media types, in Garmin's order.
    pub images: Vec<(String, String)>,
}

pub async fn activity_exports(
    pool: &Pool,
    user_id: i64,
    ids: &[Uuid],
) -> Result<Vec<ActivityExport>> {
    let mut connection = pool.get().await?;
    let rows: Vec<(Uuid, Value, Option<Value>, Option<Value>)> = activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(activities::id.eq_any(ids))
        .order((activities::started_at, activities::id))
        .select((
            activities::id,
            activities::summary,
            activities::details,
            activities::workouts,
        ))
        .load(&mut connection)
        .await?;
    let images: Vec<(Uuid, String, String)> = activity_images::table
        .filter(activity_images::activity_id.eq_any(ids))
        .order((activity_images::activity_id, activity_images::seq))
        .select((
            activity_images::activity_id,
            activity_images::image_id,
            activity_images::content_type,
        ))
        .load(&mut connection)
        .await?;
    let mut by_activity: HashMap<Uuid, Vec<(String, String)>> = HashMap::new();
    for (id, image_id, content_type) in images {
        by_activity
            .entry(id)
            .or_default()
            .push((image_id, content_type));
    }
    Ok(rows
        .into_iter()
        .map(|(id, summary, details, workouts)| ActivityExport {
            id,
            images: by_activity.remove(&id).unwrap_or_default(),
            summary,
            details,
            workouts,
        })
        .collect())
}

/// A page of the account's Garmin documents, after `after` as kind and period.
pub async fn documents(
    pool: &Pool,
    user_id: i64,
    after: Option<(String, String)>,
    limit: i64,
) -> Result<Vec<(String, String, Option<Value>)>> {
    let mut query = garmin_documents::table
        .filter(garmin_documents::user_id.eq(user_id))
        .order((garmin_documents::kind, garmin_documents::period))
        .select((
            garmin_documents::kind,
            garmin_documents::period,
            garmin_documents::data,
        ))
        .limit(limit)
        .into_boxed();
    if let Some((kind, period)) = after {
        query = query.filter(
            garmin_documents::kind
                .gt(kind.clone())
                .or(garmin_documents::kind
                    .eq(kind)
                    .and(garmin_documents::period.gt(period))),
        );
    }
    Ok(query.load(&mut pool.get().await?).await?)
}

/// The IDs of the account's activities with these Garmin IDs, by Garmin ID.
pub async fn ids_by_garmin_id(
    pool: &Pool,
    user_id: i64,
    garmin_ids: &[i64],
) -> Result<HashMap<i64, Uuid>> {
    let rows: Vec<(i64, Uuid)> = activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(activities::garmin_id.eq_any(garmin_ids))
        .select((activities::garmin_id, activities::id))
        .load(&mut pool.get().await?)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Renames the folders that older versions named by Garmin ID, once.
pub async fn rename_activity_folders(pool: &Pool, files: &FileStore) -> Result<()> {
    let rows: Vec<(i64, Uuid)> = activities::table
        .select((activities::garmin_id, activities::id))
        .load(&mut pool.get().await?)
        .await?;
    for (garmin_id, id) in rows {
        files.rename_activity(garmin_id, id).await?;
    }
    Ok(())
}

/// Whose an activity is: `None` if it is not saved, `Some(None)` if it is from
/// before accounts.
pub async fn owner(pool: &Pool, garmin_id: i64) -> Result<Option<Option<i64>>> {
    Ok(activities::table
        .filter(activities::garmin_id.eq(garmin_id))
        .select(activities::user_id)
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// How many files are moved per query, to keep memory use low.
const FILES_PER_MOVE: i64 = 20;

/// Moves original files and images that older versions kept in the database to
/// the file store. Each file is written before its column is cleared, so an
/// interrupted move continues on the next start.
pub async fn move_files_to_store(pool: &Pool, files: &FileStore) -> Result<()> {
    let mut moved = 0;
    loop {
        let originals: Vec<(Uuid, Option<Vec<u8>>)> = activities::table
            .filter(activities::original_zip.is_not_null())
            .select((activities::id, activities::original_zip))
            .limit(FILES_PER_MOVE)
            .load(&mut pool.get().await?)
            .await?;
        if originals.is_empty() {
            break;
        }
        for (id, zip) in originals {
            files.save_original(id, zip.as_deref()).await?;
            diesel::update(activities::table.find(id))
                .set(activities::original_zip.eq(None::<Vec<u8>>))
                .execute(&mut pool.get().await?)
                .await?;
            moved += 1;
        }
    }
    loop {
        let images: Vec<(Uuid, String, Option<Vec<u8>>)> = activity_images::table
            .filter(activity_images::data.is_not_null())
            .select((
                activity_images::activity_id,
                activity_images::image_id,
                activity_images::data,
            ))
            .limit(FILES_PER_MOVE)
            .load(&mut pool.get().await?)
            .await?;
        if images.is_empty() {
            break;
        }
        for (id, image_id, data) in images {
            files
                .save_image(id, &image_id, &data.unwrap_or_default())
                .await?;
            diesel::update(activity_images::table.find((id, &image_id)))
                .set(activity_images::data.eq(None::<Vec<u8>>))
                .execute(&mut pool.get().await?)
                .await?;
            moved += 1;
        }
    }
    if moved > 0 {
        tracing::info!(moved, "moved files from the database to the file store");
    }
    Ok(())
}

/// Saves the VO2 max that an activity's file holds, and marks the file as searched.
pub async fn save_vo2_max(pool: &Pool, id: Uuid, vo2_max: Option<f64>) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set((
            activities::vo2_max.eq(vo2_max),
            activities::vo2_max_checked_at.eq(diesel::dsl::now),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Reads VO2 max from the saved files of activities that were not searched yet.
pub async fn backfill_vo2_max(pool: &Pool, files: &FileStore) -> Result<()> {
    let ids: Vec<Uuid> = activities::table
        .filter(activities::vo2_max_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?;
    let mut found = 0;
    for &id in &ids {
        let vo2_max = match files.original(id).await? {
            Some(zip) => crate::track::vo2_max(&zip).unwrap_or_else(|error| {
                tracing::warn!(%id, %error, "could not read VO2 max from the file");
                None
            }),
            None => None,
        };
        found += usize::from(vo2_max.is_some());
        save_vo2_max(pool, id, vo2_max).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read VO2 max from saved files");
    }
    Ok(())
}

/// A training block: the weeks from one race to the next that is at least as long.
/// Shorter races in between do not end it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct TrainingBlock {
    /// The activity of the race that ends the block.
    pub id: Uuid,
    /// The name of that race.
    pub name: String,
    /// The local day of that race, which is the block's last day.
    pub to: NaiveDate,
    /// The day after the race that began the block. `None` for the first race of
    /// its distance, whose block starts at the beginning.
    pub from: Option<NaiveDate>,
    /// How many races the block holds, its last one included.
    pub races: usize,
}

/// One race, as the training blocks need it.
pub struct RaceDay {
    pub id: Uuid,
    pub name: String,
    pub day: NaiveDate,
    pub distance_m: f64,
}

/// The blocks that end in each of the races, oldest race first.
pub fn training_blocks(races: &[RaceDay]) -> Vec<TrainingBlock> {
    races
        .iter()
        .enumerate()
        .map(|(index, race)| {
            // Within a meter counts as the same distance.
            let start = races[..index]
                .iter()
                .rposition(|earlier| earlier.distance_m >= race.distance_m - 1.0);
            TrainingBlock {
                id: race.id,
                name: race.name.clone(),
                to: race.day,
                from: start.and_then(|start| races[start].day.succ_opt()),
                races: index - start.map_or(0, |start| start + 1) + 1,
            }
        })
        .collect()
}

/// The account's training blocks, newest first.
pub async fn training_blocks_of(pool: &Pool, user_id: i64) -> Result<Vec<TrainingBlock>> {
    #[derive(Queryable)]
    struct Row {
        id: Uuid,
        name: String,
        event: Option<String>,
        started_at: DateTime<Utc>,
        distance_m: f64,
        started_local: Option<String>,
    }
    let rows: Vec<Row> = race_results::table
        .inner_join(activities::table)
        .filter(activities::user_id.eq(user_id))
        .order((activities::started_at, activities::id))
        .select((
            activities::id,
            activities::name,
            race_results::event,
            activities::started_at,
            race_results::distance_m,
            activities::started_local,
        ))
        .load(&mut pool.get().await?)
        .await?;
    let races: Vec<RaceDay> = rows
        .into_iter()
        .map(|row| RaceDay {
            id: row.id,
            name: row.event.unwrap_or(row.name),
            day: row
                .started_local
                .and_then(|local| local.get(..10)?.parse().ok())
                .unwrap_or_else(|| row.started_at.date_naive()),
            distance_m: row.distance_m,
        })
        .collect();
    let mut blocks = training_blocks(&races);
    blocks.reverse();
    Ok(blocks)
}

/// Saves the devices that an activity's file lists, and marks the file as searched.
pub async fn save_devices(pool: &Pool, id: Uuid, devices: &[crate::track::Device]) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set((
            activities::devices.eq(serde_json::to_value(devices)?),
            activities::devices_checked_at.eq(diesel::dsl::now),
        ))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Reads the devices from the saved files of activities that were not searched yet.
pub async fn backfill_devices(pool: &Pool, files: &FileStore) -> Result<()> {
    let ids: Vec<Uuid> = activities::table
        .filter(activities::devices_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?;
    let mut found = 0;
    for &id in &ids {
        let devices = match files.original(id).await? {
            Some(zip) => crate::track::devices(&zip).unwrap_or_else(|error| {
                tracing::warn!(%id, %error, "could not read the devices from the file");
                Vec::new()
            }),
            None => Vec::new(),
        };
        found += usize::from(!devices.is_empty());
        save_devices(pool, id, &devices).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read devices from saved files");
    }
    Ok(())
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
    fn blocks_run_from_the_last_race_that_is_as_long() {
        let race = |day: &str, distance_m: f64| RaceDay {
            id: Uuid::nil(),
            name: String::new(),
            day: day.parse().unwrap(),
            distance_m,
        };
        let blocks = training_blocks(&[
            race("2025-03-01", 10_000.0),
            race("2025-04-01", 42_195.0),
            race("2025-06-01", 5_000.0),
            race("2025-07-01", 21_097.5),
            race("2025-10-01", 42_195.0),
        ]);
        let found: Vec<_> = blocks
            .iter()
            .map(|block| (block.from.map(|day| day.to_string()), block.races))
            .collect();
        assert_eq!(
            found,
            [
                (None, 1),
                (None, 2),
                (Some("2025-04-02".to_owned()), 1),
                (Some("2025-04-02".to_owned()), 2),
                (Some("2025-04-02".to_owned()), 3),
            ]
        );
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

/// A saved route without its points.
#[derive(Debug, Queryable, Selectable, serde::Serialize, utoipa::ToSchema)]
#[diesel(table_name = routes)]
pub struct RouteSummary {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    /// The activity that the route was made from, if it still exists.
    pub activity_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// About 64 points along the route, as `[latitude, longitude]`, to draw a small picture of it.
    #[schema(value_type = Vec<Vec<f64>>)]
    pub preview: Value,
}

#[derive(Insertable)]
#[diesel(table_name = routes)]
pub struct NewRoute<'a> {
    pub user_id: i64,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    pub points: &'a Value,
    pub preview: &'a Value,
    pub activity_id: Option<Uuid>,
}

pub async fn insert_route(pool: &Pool, route: &NewRoute<'_>) -> Result<RouteSummary> {
    Ok(diesel::insert_into(routes::table)
        .values(route)
        .returning(RouteSummary::as_select())
        .get_result(&mut pool.get().await?)
        .await?)
}

/// The account's routes, newest first.
pub async fn list_routes(pool: &Pool, user_id: i64) -> Result<Vec<RouteSummary>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .order((routes::created_at.desc(), routes::id.desc()))
        .select(RouteSummary::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

/// One route of the account, with its points.
pub async fn route(pool: &Pool, user_id: i64, id: Uuid) -> Result<Option<(RouteSummary, Value)>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .filter(routes::id.eq(id))
        .select((RouteSummary::as_select(), routes::points))
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// Renames a route of the account. False if it has no such route.
pub async fn update_route(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<bool> {
    let changed = diesel::update(
        routes::table
            .filter(routes::user_id.eq(user_id))
            .filter(routes::id.eq(id)),
    )
    .set((routes::name.eq(name), routes::description.eq(description)))
    .execute(&mut pool.get().await?)
    .await?;
    Ok(changed > 0)
}

/// Removes a route of the account.
pub async fn delete_route(pool: &Pool, user_id: i64, id: Uuid) -> Result<()> {
    diesel::delete(
        routes::table
            .filter(routes::user_id.eq(user_id))
            .filter(routes::id.eq(id)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

/// A route as an export holds it.
#[derive(Debug, Queryable, Selectable, serde::Serialize, serde::Deserialize)]
#[diesel(table_name = routes)]
pub struct RouteRecord {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    pub points: Value,
    pub activity_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// All of the account's routes, oldest first.
pub async fn route_records(pool: &Pool, user_id: i64) -> Result<Vec<RouteRecord>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .order((routes::created_at.asc(), routes::id.asc()))
        .select(RouteRecord::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

/// Saves a route from an export under its own ID, replacing an earlier import of
/// it. False if another account has a route with this ID. The route keeps its
/// activity only if the account has that activity.
pub async fn import_route(
    pool: &Pool,
    user_id: i64,
    route: &RouteRecord,
    preview: &Value,
) -> Result<bool> {
    let mut connection = pool.get().await?;
    let owner: Option<i64> = routes::table
        .find(route.id)
        .select(routes::user_id)
        .first(&mut connection)
        .await
        .optional()?;
    if owner.is_some_and(|owner| owner != user_id) {
        return Ok(false);
    }
    let activity_id = match route.activity_id {
        Some(id) => diesel::select(exists(
            activities::table
                .find(id)
                .filter(activities::user_id.eq(user_id)),
        ))
        .get_result::<bool>(&mut connection)
        .await?
        .then_some(id),
        None => None,
    };
    let row = (
        routes::id.eq(route.id),
        routes::user_id.eq(user_id),
        routes::name.eq(&route.name),
        routes::description.eq(&route.description),
        routes::distance_m.eq(route.distance_m),
        routes::elevation_gain_m.eq(route.elevation_gain_m),
        routes::points.eq(&route.points),
        routes::preview.eq(preview),
        routes::activity_id.eq(activity_id),
        routes::created_at.eq(route.created_at),
    );
    diesel::insert_into(routes::table)
        .values(row)
        .on_conflict(routes::id)
        .do_update()
        .set(row)
        .execute(&mut connection)
        .await?;
    Ok(true)
}
