use std::collections::HashSet;

use anyhow::Result;
use diesel::dsl::{count_star, exists, sum};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::Value;
use uuid::Uuid;

use crate::database::Pool;
use crate::models::{ActivityRecord, ActivitySummary, Filter, Order, Totals};
use crate::schema::{activities, track_points};

/// The activities that match, for the heatmap to pick from its tracks.
pub async fn matching_ids(pool: &Pool, filter: &Filter) -> Result<HashSet<Uuid>> {
    let ids: Vec<Uuid> = filter
        .apply(activities::table.select(activities::id).into_boxed())
        .load(&mut pool.get().await?)
        .await?;
    Ok(ids.into_iter().collect())
}

/// The first activities that match in this order, after skipping `offset` of them.
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
