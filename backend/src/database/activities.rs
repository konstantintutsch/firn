use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::dsl::min;
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::RunQueryDsl;
use serde_json::Value;
use uuid::Uuid;

use super::Pool;
use crate::garmin::Activity;
use crate::schema::activities;

mod export;
mod filter;
mod search;

pub use export::*;
pub use filter::*;
pub use search::*;

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

/// The first day of saved activities in local time, where the health data starts.
pub async fn first_activity_day(pool: &Pool, user_id: i64) -> Result<Option<NaiveDate>> {
    let first: Option<String> = activities::table
        .filter(activities::user_id.eq(user_id))
        .select(min(activities::started_local))
        .first(&mut pool.get().await?)
        .await?;
    Ok(first.and_then(|text| NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()))
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

/// Deletes some of the account's activities, with their tracks, photos and results.
pub async fn delete_activities(pool: &Pool, user_id: i64, ids: &[Uuid]) -> Result<()> {
    diesel::delete(
        activities::table
            .filter(activities::user_id.eq(user_id))
            .filter(activities::id.eq_any(ids)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
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

/// The Garmin ID and the ID of every activity, of all accounts.
pub async fn all_garmin_ids(pool: &Pool) -> Result<Vec<(i64, Uuid)>> {
    Ok(activities::table
        .select((activities::garmin_id, activities::id))
        .load(&mut pool.get().await?)
        .await?)
}

/// Some of the original files that older versions kept in the database.
pub async fn stored_originals(pool: &Pool, limit: i64) -> Result<Vec<(Uuid, Option<Vec<u8>>)>> {
    Ok(activities::table
        .filter(activities::original_zip.is_not_null())
        .select((activities::id, activities::original_zip))
        .limit(limit)
        .load(&mut pool.get().await?)
        .await?)
}

/// Removes an activity's original file from the database.
pub async fn clear_stored_original(pool: &Pool, id: Uuid) -> Result<()> {
    diesel::update(activities::table.find(id))
        .set(activities::original_zip.eq(None::<Vec<u8>>))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// The activities with a track whose file was not searched for a VO2 max yet.
pub async fn without_vo2_max_check(pool: &Pool) -> Result<Vec<Uuid>> {
    Ok(activities::table
        .filter(activities::vo2_max_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?)
}

/// The activities with a track whose file was not searched for devices yet.
pub async fn without_devices_check(pool: &Pool) -> Result<Vec<Uuid>> {
    Ok(activities::table
        .filter(activities::devices_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?)
}
