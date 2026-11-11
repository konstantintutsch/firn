use std::collections::HashSet;

use anyhow::Result;
use chrono::NaiveDate;
use diesel::prelude::*;
use diesel::upsert::excluded;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::Value;

use super::Pool;
use crate::database::schema::{daily_metrics, garmin_documents};

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
                        daily_metrics::seq,
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

/// The kind and period of each of the account's health documents.
pub async fn document_keys(pool: &Pool, user_id: i64) -> Result<Vec<(String, String)>> {
    Ok(garmin_documents::table
        .filter(garmin_documents::user_id.eq(user_id))
        .select((garmin_documents::kind, garmin_documents::period))
        .load(&mut pool.get().await?)
        .await?)
}

/// Deletes the account's health documents of one kind, for some periods.
pub async fn delete_documents(
    pool: &Pool,
    user_id: i64,
    kind: &str,
    periods: &[String],
) -> Result<()> {
    diesel::delete(
        garmin_documents::table
            .filter(garmin_documents::user_id.eq(user_id))
            .filter(garmin_documents::kind.eq(kind))
            .filter(garmin_documents::period.eq_any(periods)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
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
