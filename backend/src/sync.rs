use std::time::Duration;

use crate::db::{self, Pool};
use crate::garmin::{Activity, GarminClient, GarminError};
use crate::track;

/// The default page size of Garmin's activity search.
const PAGE_SIZE: u32 = 99;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub listed: usize,
    pub to_download: usize,
    pub downloaded: usize,
    pub failed: usize,
}

/// Saves every run and its track. Safe to interrupt and rerun: finished runs are
/// skipped, and only a failed or new run costs a download.
pub async fn sync_runs(
    pool: &Pool,
    client: &mut GarminClient,
    pause: Duration,
    on_progress: impl Fn(&Report),
) -> Result<Report, GarminError> {
    let mut report = Report::default();
    let mut start = 0;
    let mut activities = Vec::new();
    loop {
        let page = client.list_runs(start, PAGE_SIZE).await?;
        if page.is_empty() {
            break;
        }
        db::upsert_runs(pool, &page).await?;
        start += page.len() as u32;
        activities.extend(page);
    }
    report.listed = activities.len();
    tracing::info!(runs = report.listed, "listed runs");

    let mut pending = Vec::new();
    for activity in &activities {
        if db::is_track_synced(pool, activity.id).await? {
            continue;
        }
        if activity.has_track {
            pending.push(activity);
        } else {
            db::save_track(pool, activity.id, None, &[]).await?;
        }
    }
    report.to_download = pending.len();
    on_progress(&report);

    for activity in pending {
        match save_original(pool, client, activity).await {
            Ok(false) => {
                report.to_download -= 1;
                tracing::warn!(id = activity.id, "Garmin has no file for this run");
            }
            Ok(true) => {
                report.downloaded += 1;
                tracing::info!(id = activity.id, done = report.downloaded, "saved track");
            }
            // Later downloads would fail the same way, so stop here.
            Err(
                error @ (GarminError::RateLimited
                | GarminError::InvalidApiKey
                | GarminError::Database(_)),
            ) => return Err(error),
            Err(error) => {
                report.failed += 1;
                tracing::warn!(id = activity.id, %error, "skipped track, next sync retries it");
            }
        }
        on_progress(&report);
        tokio::time::sleep(pause).await;
    }
    Ok(report)
}

/// Returns whether Garmin had a file to save.
async fn save_original(
    pool: &Pool,
    client: &mut GarminClient,
    activity: &Activity,
) -> Result<bool, GarminError> {
    let zip = match client.download_original(activity.id).await {
        Ok(zip) => zip,
        Err(GarminError::NotFound) => {
            db::save_track(pool, activity.id, None, &[]).await?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let points = track::parse_archive(&zip)
        .map_err(|error| GarminError::Unexpected(format!("activity file: {error:#}")))?;
    db::save_track(pool, activity.id, Some(&zip), &points).await?;
    Ok(true)
}
