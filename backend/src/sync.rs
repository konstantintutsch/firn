use std::time::Duration;

use crate::db::{self, Pool};
use crate::garmin::{Activity, GarminClient, GarminError};
use crate::gpx;

const PAGE_SIZE: u32 = 100;

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub listed: usize,
    pub downloaded: usize,
    pub failed: usize,
}

/// Saves every run and its track. Safe to interrupt and rerun: finished runs are
/// skipped, and only a failed or new run costs a download.
pub async fn sync_runs(
    pool: &Pool,
    client: &mut GarminClient,
    pause: Duration,
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

    for activity in &activities {
        if db::is_track_synced(pool, activity.id).await? {
            continue;
        }
        if !activity.has_track {
            db::save_track(pool, activity.id, None, &[]).await?;
            continue;
        }
        match save_gpx(pool, client, activity).await {
            Ok(false) => tracing::warn!(id = activity.id, "Garmin has no track for this run"),
            Ok(true) => {
                report.downloaded += 1;
                tracing::info!(id = activity.id, done = report.downloaded, "saved track");
            }
            // Retrying these immediately only makes things worse, so stop here.
            Err(
                error @ (GarminError::RateLimited
                | GarminError::Blocked
                | GarminError::NeedsLogin
                | GarminError::Database(_)),
            ) => return Err(error),
            Err(error) => {
                report.failed += 1;
                tracing::warn!(id = activity.id, %error, "skipped track, next sync retries it");
            }
        }
        tokio::time::sleep(pause).await;
    }
    Ok(report)
}

/// Returns whether Garmin had a track to save.
async fn save_gpx(
    pool: &Pool,
    client: &mut GarminClient,
    activity: &Activity,
) -> Result<bool, GarminError> {
    let gpx = match client.download_gpx(activity.id).await {
        Ok(gpx) => gpx,
        Err(GarminError::NotFound) => {
            db::save_track(pool, activity.id, None, &[]).await?;
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let points = gpx::parse_track(&gpx)
        .map_err(|error| GarminError::Unexpected(format!("activity GPX: {error}")))?;
    db::save_track(pool, activity.id, Some(&gpx), &points).await?;
    Ok(true)
}
