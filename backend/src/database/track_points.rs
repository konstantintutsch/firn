use anyhow::Result;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::json;
use uuid::Uuid;

use super::Pool;
use crate::database::schema::{activities, track_points};
use crate::heatmap::{self, Viewport};
use crate::models::ActivityTrack;
use crate::track::Point;

/// Postgres allows 65535 bind parameters per statement, and a point uses 4.
const POINTS_PER_INSERT: usize = 10_000;

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
