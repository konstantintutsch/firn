use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, Utc};
use diesel::dsl::{count_star, exists, sum};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::Value;
use uuid::Uuid;

use super::Filter;
use crate::database::Pool;
use crate::schema::{activities, track_points};

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
