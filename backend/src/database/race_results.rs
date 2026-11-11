use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use diesel::dsl::exists;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::Pool;
use crate::models::{RaceDay, RaceResult};
use crate::schema::{activities, race_results};

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

/// The account's races that have a result, oldest first.
pub async fn race_days(pool: &Pool, user_id: i64) -> Result<Vec<RaceDay>> {
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
    Ok(rows
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
        .collect())
}
