use std::collections::HashSet;
use std::time::Duration;

use chrono::{Days, NaiveDate};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::db::{self, Pool};
use crate::files::FileStore;
use crate::garmin::{self, Activity, GarminClient, GarminError};
use crate::track;
use crate::wellness::{self, KINDS, Kind, Period};

/// The default page size of Garmin's activity search.
const PAGE_SIZE: u32 = 99;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub listed: usize,
    pub to_download: usize,
    pub downloaded: usize,
    pub failed: usize,
    /// Health data requests that this sync has to make.
    pub health_to_fetch: usize,
    pub health_fetched: usize,
    pub health_failed: usize,
}

/// Saves every activity, its original file, details, workouts and images, then the health data.
/// Safe to interrupt and rerun: finished work is skipped.
pub async fn sync_all(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    user_id: i64,
    health: HealthRange,
    pause: Duration,
    on_progress: impl Fn(&Report),
) -> Result<Report, GarminError> {
    let mut report = sync_activities(pool, files, client, user_id, pause, &on_progress).await?;
    sync_health(
        pool,
        client,
        user_id,
        health,
        pause,
        &mut report,
        &on_progress,
    )
    .await?;
    Ok(report)
}

/// Saves every activity with its original file, details, workouts and images. Activities without
/// GPS are saved too, for the statistics, but only GPS tracks go on the map.
pub async fn sync_activities(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    user_id: i64,
    pause: Duration,
    on_progress: impl Fn(&Report),
) -> Result<Report, GarminError> {
    let mut report = Report::default();
    let mut start = 0;
    let mut activities = Vec::new();
    loop {
        let page = client.list_activities(start, PAGE_SIZE).await?;
        if page.is_empty() {
            break;
        }
        start += page.len() as u32;
        db::upsert_activities(pool, user_id, &page).await?;
        activities.extend(page);
    }
    report.listed = activities.len();
    tracing::info!(user_id, activities = report.listed, "listed activities");
    db::skip_activities_without_images(pool, user_id).await?;

    let ids: Vec<i64> = activities.iter().map(|activity| activity.id).collect();
    let mut missing = db::missing_many(pool, user_id, &ids).await?;
    let mut pending = Vec::new();
    for activity in &activities {
        if let Some((id, missing)) = missing.remove(&activity.id)
            && (missing.track || missing.details || missing.workouts || missing.images)
        {
            pending.push((activity, id, missing));
        }
    }
    report.to_download = pending.len();
    on_progress(&report);

    for (activity, id, missing) in pending {
        let result = download_activity(pool, files, client, activity, id, &missing).await;
        match result {
            Ok(()) => {
                report.downloaded += 1;
                tracing::info!(id = activity.id, done = report.downloaded, "saved activity");
            }
            Err(error) if stops_sync(&error) => return Err(error),
            Err(error) => {
                report.failed += 1;
                tracing::warn!(id = activity.id, %error, "skipped activity, next sync retries it");
            }
        }
        on_progress(&report);
        tokio::time::sleep(pause).await;
    }
    Ok(report)
}

/// Downloads what is missing of one activity: its file and track, details, workouts and images.
async fn download_activity(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    activity: &Activity,
    id: Uuid,
    missing: &db::Missing,
) -> Result<(), GarminError> {
    if missing.track {
        save_original(pool, files, client, activity, id).await?;
    }
    let mut details = None;
    if missing.details {
        let fetched = client
            .activity_details(activity.id)
            .await?
            .unwrap_or_else(|| json!({}));
        db::save_details(pool, id, &fetched).await?;
        details = Some(fetched);
    }
    if missing.workouts {
        let workouts = client.activity_workouts(activity.id).await?;
        db::save_workouts(pool, id, &workouts.unwrap_or_else(|| json!([]))).await?;
    }
    if missing.images {
        save_images(pool, files, client, id, activity.id, details).await?;
    }
    Ok(())
}

/// Downloads everything of one saved activity again, for example after it was
/// edited on Garmin Connect. `None` if the account has no such activity.
pub async fn resync_activity(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    user_id: i64,
    id: Uuid,
) -> Result<Option<()>, GarminError> {
    let Some(summary) = db::summary(pool, user_id, id).await? else {
        return Ok(None);
    };
    let activity = Activity::from_summary(summary)?;
    let everything = db::Missing {
        track: true,
        details: true,
        workouts: true,
        images: true,
    };
    download_activity(pool, files, client, &activity, id, &everything).await?;
    Ok(Some(()))
}

/// Later requests would fail the same way, so the sync stops.
fn stops_sync(error: &GarminError) -> bool {
    matches!(
        error,
        GarminError::RateLimited | GarminError::InvalidApiKey | GarminError::Database(_)
    )
}

async fn save_original(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    activity: &Activity,
    id: Uuid,
) -> Result<(), GarminError> {
    let zip = match client.download_original(activity.id).await {
        Ok(zip) => zip,
        Err(GarminError::NotFound) => {
            tracing::warn!(id = activity.id, "Garmin has no file for this activity");
            files.save_original(id, None).await?;
            db::save_track(pool, id, &[]).await?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let points = if activity.has_track {
        track::parse_archive(&zip)
            .map_err(|error| GarminError::Unexpected(format!("activity file: {error:#}")))?
    } else {
        Vec::new()
    };
    // The file first: a saved track marks the activity as done.
    files.save_original(id, Some(&zip)).await?;
    db::save_track(pool, id, &points).await?;
    let vo2_max = track::vo2_max(&zip).unwrap_or_else(|error| {
        tracing::warn!(id = activity.id, %error, "could not read VO2 max from the file");
        None
    });
    db::save_vo2_max(pool, id, vo2_max).await?;
    let devices = track::devices(&zip).unwrap_or_else(|error| {
        tracing::warn!(id = activity.id, %error, "could not read the devices from the file");
        Vec::new()
    });
    db::save_devices(pool, id, &devices).await?;
    Ok(())
}

/// Downloads the photos that the details list. `fresh` are details fetched just now.
async fn save_images(
    pool: &Pool,
    files: &FileStore,
    client: &mut GarminClient,
    id: Uuid,
    garmin_id: i64,
    fresh: Option<Value>,
) -> Result<(), GarminError> {
    let details = match fresh {
        Some(details) => details,
        None => {
            let saved = db::details(pool, id).await?.unwrap_or_default();
            if garmin::activity_images(&saved).is_empty() {
                saved
            } else {
                // The links in saved details expire after a day, so this asks for new ones.
                let details = client
                    .activity_details(garmin_id)
                    .await?
                    .unwrap_or_else(|| json!({}));
                db::save_details(pool, id, &details).await?;
                details
            }
        }
    };
    let mut saved = Vec::new();
    let mut downloads = Vec::new();
    for image in garmin::activity_images(&details) {
        let (data, content_type) = match client.download_image(&image.url).await {
            Ok(download) => download,
            // Garmin lists older images with links that do not work, and retrying does not help.
            Err(error @ (GarminError::Forbidden | GarminError::NotFound)) => {
                tracing::warn!(%id, image = image.id, %error, "skipped image");
                continue;
            }
            Err(error) => return Err(error),
        };
        downloads.push((image.id.clone(), data));
        saved.push(db::Image {
            id: image.id,
            content_type,
        });
    }
    let downloads: Vec<(&str, &[u8])> = downloads
        .iter()
        .map(|(image_id, data)| (image_id.as_str(), data.as_slice()))
        .collect();
    files.save_images(id, &downloads).await?;
    db::save_images(pool, id, &saved).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct HealthRange {
    pub today: NaiveDate,
    /// How far back health data goes at least. It also goes back to the first activity.
    pub history_days: u64,
}

struct Request {
    kind: &'static Kind,
    /// Where the response is saved.
    period: String,
    /// What is asked for, which ends today at the latest.
    asked: String,
    day: Option<NaiveDate>,
}

/// Saves all health data from the first activity on, newest first, so that an
/// interrupted sync already shows recent days.
pub async fn sync_health(
    pool: &Pool,
    client: &mut GarminClient,
    user_id: i64,
    range: HealthRange,
    pause: Duration,
    report: &mut Report,
    on_progress: impl Fn(&Report),
) -> Result<(), GarminError> {
    let today = range.today;
    let default_first = today - Days::new(range.history_days);
    let first = db::first_activity_day(pool, user_id)
        .await?
        .map_or(default_first, |day| day.min(default_first));
    let requests = health_requests(pool, user_id, first, today).await?;
    report.health_to_fetch = requests.len();
    on_progress(report);

    // An API key may not reach some data, like the cycle of an account without one.
    let mut refused: HashSet<&str> = HashSet::new();
    for request in requests {
        if refused.contains(request.kind.name) {
            report.health_to_fetch -= 1;
            continue;
        }
        let (path, query) = request.kind.request(&request.asked);
        match client.get_json(&path, &query).await {
            Ok(data) => {
                let metrics = data
                    .as_ref()
                    .map(|data| request.kind.extract(data, request.day))
                    .unwrap_or_default();
                db::save_document(
                    pool,
                    user_id,
                    request.kind.name,
                    &request.period,
                    data.as_ref(),
                    &metrics,
                )
                .await?;
                report.health_fetched += 1;
            }
            Err(error) if stops_sync(&error) => return Err(error),
            Err(GarminError::Forbidden) => {
                tracing::info!(kind = request.kind.name, "Garmin refused this data");
                refused.insert(request.kind.name);
                report.health_to_fetch -= 1;
            }
            Err(error) => {
                report.health_failed += 1;
                tracing::warn!(
                    kind = request.kind.name,
                    period = request.period,
                    %error,
                    "skipped health data, next sync retries it"
                );
            }
        }
        on_progress(report);
        tokio::time::sleep(pause).await;
    }
    Ok(())
}

async fn health_requests(
    pool: &Pool,
    user_id: i64,
    first: NaiveDate,
    today: NaiveDate,
) -> Result<Vec<Request>, GarminError> {
    let recent = today - Days::new(wellness::REFRESH_DAYS);
    let mut requests: Vec<(NaiveDate, Request)> = Vec::new();
    for kind in KINDS {
        let saved = db::document_periods(pool, user_id, kind.name).await?;
        let mut add = |sort: NaiveDate, period: String, asked: String, day: Option<NaiveDate>| {
            requests.push((
                sort,
                Request {
                    kind,
                    period,
                    asked,
                    day,
                },
            ));
        };
        match kind.period {
            Period::Undated => add(today, String::new(), String::new(), None),
            Period::Today => add(today, today.to_string(), today.to_string(), Some(today)),
            Period::Range => {
                for (start, end) in wellness::ranges(first, today) {
                    let period = wellness::range_period(start, end);
                    if end >= recent || !saved.contains(&period) {
                        let asked = wellness::range_period(start, end.min(today));
                        add(end, period, asked, None);
                    }
                }
            }
            Period::Day => {
                for day in first.iter_days().take_while(|day| *day <= today) {
                    if day >= recent || !saved.contains(&day.to_string()) {
                        add(day, day.to_string(), day.to_string(), Some(day));
                    }
                }
            }
        }
    }
    // Newest first, and for one day the summaries before the details.
    requests.sort_by_key(|(day, _)| std::cmp::Reverse(*day));
    Ok(requests.into_iter().map(|(_, request)| request).collect())
}
