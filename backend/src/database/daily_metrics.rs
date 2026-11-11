use std::collections::HashSet;

use anyhow::Result;
use chrono::NaiveDate;
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::json;
use uuid::Uuid;

use super::Pool;
use crate::models::Vo2Reading;
use crate::schema::{activities, daily_metrics, garmin_documents};

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
        .order((
            daily_metrics::metric,
            daily_metrics::day,
            daily_metrics::seq,
        ))
        .into_boxed();
    if let Some(from) = from {
        query = query.filter(daily_metrics::day.ge(from));
    }
    if let Some(to) = to {
        query = query.filter(daily_metrics::day.le(to));
    }
    Ok(query.load(&mut pool.get().await?).await?)
}

/// Saves a day of values from Apple Health as a document of its own, so that an
/// export holds them. Values that Garmin or an earlier import saved stay. Every
/// VO2 max of the day is kept, numbered in the order given, each with its
/// activity if that is one of the account's.
pub async fn save_apple_health_day(
    pool: &Pool,
    user_id: i64,
    day: NaiveDate,
    metrics: &std::collections::BTreeMap<String, f64>,
    vo2_max: &[Vo2Reading],
) -> Result<()> {
    let mut connection = pool.get().await?;
    // A link to an activity that is not the account's, or not here, is left out.
    let wanted: Vec<Uuid> = vo2_max
        .iter()
        .filter_map(|reading| reading.activity_id)
        .collect();
    let known: HashSet<Uuid> = activities::table
        .filter(activities::user_id.eq(user_id))
        .filter(activities::id.eq_any(&wanted))
        .select(activities::id)
        .load(&mut connection)
        .await?
        .into_iter()
        .collect();
    let vo2_max: Vec<Vo2Reading> = vo2_max
        .iter()
        .map(|reading| Vo2Reading {
            value: reading.value,
            activity_id: reading.activity_id.filter(|id| known.contains(id)),
        })
        .collect();

    let rows: Vec<_> = metrics
        .iter()
        .map(|(metric, value)| {
            (
                daily_metrics::user_id.eq(user_id),
                daily_metrics::day.eq(day),
                daily_metrics::metric.eq(metric.as_str()),
                daily_metrics::value.eq(*value),
            )
        })
        .collect();
    let vo2_rows: Vec<_> = vo2_max
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            (
                daily_metrics::user_id.eq(user_id),
                daily_metrics::day.eq(day),
                daily_metrics::metric.eq(VO2_MAX_METRIC),
                daily_metrics::value.eq(reading.value),
                daily_metrics::seq.eq(index as i32),
                daily_metrics::activity_id.eq(reading.activity_id),
            )
        })
        .collect();
    let mut data = json!({ "metrics": metrics });
    if !vo2_max.is_empty() {
        data["vo2_max"] = serde_json::to_value(&vo2_max)?;
    }
    connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            diesel::insert_into(garmin_documents::table)
                .values((
                    garmin_documents::user_id.eq(user_id),
                    garmin_documents::kind.eq(crate::apple_health::DOCUMENT_KIND),
                    garmin_documents::period.eq(day.to_string()),
                    garmin_documents::data.eq(&data),
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
                    .on_conflict_do_nothing()
                    .execute(&mut *connection)
                    .await?;
            }
            // Only Apple Health has these, so an import may update its own.
            if !vo2_rows.is_empty() {
                diesel::insert_into(daily_metrics::table)
                    .values(&vo2_rows)
                    .on_conflict((
                        daily_metrics::user_id,
                        daily_metrics::metric,
                        daily_metrics::day,
                        daily_metrics::seq,
                    ))
                    .do_update()
                    .set((
                        daily_metrics::value.eq(excluded(daily_metrics::value)),
                        daily_metrics::activity_id.eq(excluded(daily_metrics::activity_id)),
                    ))
                    .execute(&mut *connection)
                    .await?;
            }
            Ok(())
        })
        .await?;
    Ok(())
}

/// The name of the daily metric that holds the VO2 max that Apple Health measured.
pub const VO2_MAX_METRIC: &str = "vo2_max";

/// The metric and day of each of the account's daily health values.
pub async fn metric_keys(pool: &Pool, user_id: i64) -> Result<Vec<(String, NaiveDate)>> {
    Ok(daily_metrics::table
        .filter(daily_metrics::user_id.eq(user_id))
        .select((daily_metrics::metric, daily_metrics::day))
        .load(&mut pool.get().await?)
        .await?)
}

/// Deletes the account's values of one metric, for some days.
pub async fn delete_metrics(
    pool: &Pool,
    user_id: i64,
    metric: &str,
    days: &[NaiveDate],
) -> Result<()> {
    diesel::delete(
        daily_metrics::table
            .filter(daily_metrics::user_id.eq(user_id))
            .filter(daily_metrics::metric.eq(metric))
            .filter(daily_metrics::day.eq_any(days)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}
