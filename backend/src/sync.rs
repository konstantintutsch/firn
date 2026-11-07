use std::collections::HashSet;
use std::time::Duration;

use chrono::{Days, NaiveDate};
use serde_json::json;

use crate::db::{self, Pool};
use crate::garmin::{Activity, GarminClient, GarminError};
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

/// Saves every activity, its original file, details and workouts, then the health data.
/// Safe to interrupt and rerun: finished work is skipped.
pub async fn sync_all(
    pool: &Pool,
    client: &mut GarminClient,
    user_id: i64,
    health: HealthRange,
    pause: Duration,
    on_progress: impl Fn(&Report),
) -> Result<Report, GarminError> {
    let mut report = sync_activities(pool, client, user_id, pause, &on_progress).await?;
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

/// Saves every activity with its original file, details and workouts. Activities without
/// GPS are saved too, for the statistics, but only GPS tracks go on the map.
pub async fn sync_activities(
    pool: &Pool,
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

    let mut pending = Vec::new();
    for activity in &activities {
        if let Some(missing) = db::missing(pool, user_id, activity.id).await?
            && (missing.track || missing.details || missing.workouts)
        {
            pending.push((activity, missing));
        }
    }
    report.to_download = pending.len();
    on_progress(&report);

    for (activity, missing) in pending {
        let result = async {
            if missing.track {
                save_original(pool, client, activity).await?;
            }
            if missing.details {
                let details = client.activity_details(activity.id).await?;
                db::save_details(pool, activity.id, &details.unwrap_or_else(|| json!({}))).await?;
            }
            if missing.workouts {
                let workouts = client.activity_workouts(activity.id).await?;
                db::save_workouts(pool, activity.id, &workouts.unwrap_or_else(|| json!([])))
                    .await?;
            }
            Ok(())
        }
        .await;
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

/// Later requests would fail the same way, so the sync stops.
fn stops_sync(error: &GarminError) -> bool {
    matches!(
        error,
        GarminError::RateLimited | GarminError::InvalidApiKey | GarminError::Database(_)
    )
}

async fn save_original(
    pool: &Pool,
    client: &mut GarminClient,
    activity: &Activity,
) -> Result<(), GarminError> {
    let zip = match client.download_original(activity.id).await {
        Ok(zip) => zip,
        Err(GarminError::NotFound) => {
            tracing::warn!(id = activity.id, "Garmin has no file for this activity");
            db::save_track(pool, activity.id, None, &[]).await?;
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
    db::save_track(pool, activity.id, Some(&zip), &points).await?;
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
