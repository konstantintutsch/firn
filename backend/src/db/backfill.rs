use anyhow::Result;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::{Pool, save_devices, save_vo2_max};
use crate::files::FileStore;
use crate::schema::{activities, activity_images};

/// Renames the folders that older versions named by Garmin ID, once.
pub async fn rename_activity_folders(pool: &Pool, files: &FileStore) -> Result<()> {
    let rows: Vec<(i64, Uuid)> = activities::table
        .select((activities::garmin_id, activities::id))
        .load(&mut pool.get().await?)
        .await?;
    for (garmin_id, id) in rows {
        files.rename_activity(garmin_id, id).await?;
    }
    Ok(())
}

/// How many files are moved per query, to keep memory use low.
const FILES_PER_MOVE: i64 = 20;

/// Moves original files and images that older versions kept in the database to
/// the file store. Each file is written before its column is cleared, so an
/// interrupted move continues on the next start.
pub async fn move_files_to_store(pool: &Pool, files: &FileStore) -> Result<()> {
    let mut moved = 0;
    loop {
        let originals: Vec<(Uuid, Option<Vec<u8>>)> = activities::table
            .filter(activities::original_zip.is_not_null())
            .select((activities::id, activities::original_zip))
            .limit(FILES_PER_MOVE)
            .load(&mut pool.get().await?)
            .await?;
        if originals.is_empty() {
            break;
        }
        for (id, zip) in originals {
            files.save_original(id, zip.as_deref()).await?;
            diesel::update(activities::table.find(id))
                .set(activities::original_zip.eq(None::<Vec<u8>>))
                .execute(&mut pool.get().await?)
                .await?;
            moved += 1;
        }
    }
    loop {
        let images: Vec<(Uuid, String, Option<Vec<u8>>)> = activity_images::table
            .filter(activity_images::data.is_not_null())
            .select((
                activity_images::activity_id,
                activity_images::image_id,
                activity_images::data,
            ))
            .limit(FILES_PER_MOVE)
            .load(&mut pool.get().await?)
            .await?;
        if images.is_empty() {
            break;
        }
        for (id, image_id, data) in images {
            files
                .save_image(id, &image_id, &data.unwrap_or_default())
                .await?;
            diesel::update(activity_images::table.find((id, &image_id)))
                .set(activity_images::data.eq(None::<Vec<u8>>))
                .execute(&mut pool.get().await?)
                .await?;
            moved += 1;
        }
    }
    if moved > 0 {
        tracing::info!(moved, "moved files from the database to the file store");
    }
    Ok(())
}

/// Reads VO2 max from the saved files of activities that were not searched yet.
pub async fn backfill_vo2_max(pool: &Pool, files: &FileStore) -> Result<()> {
    let ids: Vec<Uuid> = activities::table
        .filter(activities::vo2_max_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?;
    let mut found = 0;
    for &id in &ids {
        let vo2_max = match files.original(id).await? {
            Some(zip) => crate::track::vo2_max(&zip).unwrap_or_else(|error| {
                tracing::warn!(%id, %error, "could not read VO2 max from the file");
                None
            }),
            None => None,
        };
        found += usize::from(vo2_max.is_some());
        save_vo2_max(pool, id, vo2_max).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read VO2 max from saved files");
    }
    Ok(())
}

/// Reads the devices from the saved files of activities that were not searched yet.
pub async fn backfill_devices(pool: &Pool, files: &FileStore) -> Result<()> {
    let ids: Vec<Uuid> = activities::table
        .filter(activities::devices_checked_at.is_null())
        .filter(activities::track_synced_at.is_not_null())
        .select(activities::id)
        .load(&mut pool.get().await?)
        .await?;
    let mut found = 0;
    for &id in &ids {
        let devices = match files.original(id).await? {
            Some(zip) => crate::track::devices(&zip).unwrap_or_else(|error| {
                tracing::warn!(%id, %error, "could not read the devices from the file");
                Vec::new()
            }),
            None => Vec::new(),
        };
        found += usize::from(!devices.is_empty());
        save_devices(pool, id, &devices).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read devices from saved files");
    }
    Ok(())
}
