use std::collections::HashMap;

use anyhow::Result;
use diesel::dsl::exists;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::Pool;
use crate::schema::{activities, race_results};

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
