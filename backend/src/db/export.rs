use std::collections::HashMap;

use anyhow::Result;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::Value;
use uuid::Uuid;

use super::Pool;
use crate::schema::{activities, activity_images};

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
