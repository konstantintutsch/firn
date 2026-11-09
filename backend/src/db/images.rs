use anyhow::Result;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, RunQueryDsl};
use uuid::Uuid;

use super::Pool;
use crate::schema::{activities, activity_images};

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
