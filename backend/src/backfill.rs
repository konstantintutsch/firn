//! Brings what older versions saved into today's shape, on every start.

use anyhow::Result;

use crate::database::{self, Pool};
use crate::files::FileStore;

/// Renames the folders that older versions named by Garmin ID, once.
pub async fn rename_activity_folders(pool: &Pool, files: &FileStore) -> Result<()> {
    for (garmin_id, id) in database::all_garmin_ids(pool).await? {
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
        let originals = database::stored_originals(pool, FILES_PER_MOVE).await?;
        if originals.is_empty() {
            break;
        }
        for (id, zip) in originals {
            files.save_original(id, zip.as_deref()).await?;
            database::clear_stored_original(pool, id).await?;
            moved += 1;
        }
    }
    loop {
        let images = database::stored_images(pool, FILES_PER_MOVE).await?;
        if images.is_empty() {
            break;
        }
        for (id, image_id, data) in images {
            files
                .save_image(id, &image_id, &data.unwrap_or_default())
                .await?;
            database::clear_stored_image(pool, id, &image_id).await?;
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
    let ids = database::without_vo2_max_check(pool).await?;
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
        database::save_vo2_max(pool, id, vo2_max).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read VO2 max from saved files");
    }
    Ok(())
}

/// Reads the devices from the saved files of activities that were not searched yet.
pub async fn backfill_devices(pool: &Pool, files: &FileStore) -> Result<()> {
    let ids = database::without_devices_check(pool).await?;
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
        database::save_devices(pool, id, &devices).await?;
    }
    if !ids.is_empty() {
        tracing::info!(searched = ids.len(), found, "read devices from saved files");
    }
    Ok(())
}
