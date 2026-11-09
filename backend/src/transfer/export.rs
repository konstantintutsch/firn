use std::io::{Seek, Write};

use anyhow::Result;
use tokio::runtime::Handle;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::format::{
    ExportedActivity, ExportedDocument, ExportedImage, FORMAT_VERSION, MARKER, Marker,
};
use super::{ACTIVITIES_PER_QUERY, DOCUMENTS_PER_QUERY};
use crate::db::{self, Pool};
use crate::files::FileStore;

/// Writes all of the account's data as a ZIP to `out`.
pub fn export(
    handle: &Handle,
    pool: &Pool,
    files: &FileStore,
    user_id: i64,
    out: impl Write + Seek,
) -> Result<()> {
    let mut zip = ZipWriter::new(out);
    // Original files are ZIPs and photos are compressed already.
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default();

    zip.start_file(MARKER, deflated)?;
    serde_json::to_writer_pretty(
        &mut zip,
        &Marker {
            format: "firn-export".to_owned(),
            version: FORMAT_VERSION,
            exported_at: chrono::Utc::now(),
        },
    )?;

    let ids = handle.block_on(db::activity_ids(pool, user_id))?;
    for chunk in ids.chunks(ACTIVITIES_PER_QUERY) {
        let mut results = handle.block_on(db::race_results(pool, chunk))?;
        for activity in handle.block_on(db::activity_exports(pool, user_id, chunk))? {
            let dir = format!("activities/{}", activity.id);
            zip.start_file(format!("{dir}/activity.json"), deflated)?;
            serde_json::to_writer(
                &mut zip,
                &ExportedActivity {
                    id: Some(activity.id),
                    summary: activity.summary,
                    details: activity.details,
                    workouts: activity.workouts,
                    images: activity
                        .images
                        .iter()
                        .map(|(id, content_type)| ExportedImage {
                            id: id.clone(),
                            content_type: content_type.clone(),
                        })
                        .collect(),
                    result: results.remove(&activity.id),
                },
            )?;
            if let Some(original) = handle.block_on(files.original(activity.id))? {
                zip.start_file(format!("{dir}/original.zip"), stored)?;
                zip.write_all(&original)?;
            }
            for (index, (image_id, _)) in activity.images.iter().enumerate() {
                if let Some(image) = handle.block_on(files.image(activity.id, image_id))? {
                    zip.start_file(format!("{dir}/images/{index}"), stored)?;
                    zip.write_all(&image)?;
                }
            }
        }
    }

    for route in handle.block_on(db::route_records(pool, user_id))? {
        zip.start_file(format!("routes/{}.json", route.id), deflated)?;
        serde_json::to_writer(&mut zip, &route)?;
    }

    zip.start_file("health.jsonl", deflated)?;
    let mut after = None;
    loop {
        let page = handle.block_on(db::documents(pool, user_id, after, DOCUMENTS_PER_QUERY))?;
        let Some((kind, period, _)) = page.last() else {
            break;
        };
        after = Some((kind.clone(), period.clone()));
        for (kind, period, data) in page {
            serde_json::to_writer(&mut zip, &ExportedDocument { kind, period, data })?;
            zip.write_all(b"\n")?;
        }
    }
    zip.finish()?;
    Ok(())
}
