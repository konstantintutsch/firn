use std::collections::HashSet;
use std::time::Duration;

use chrono::{Days, NaiveDate};

use crate::db::{self, Pool};
use crate::garmin::{GarminClient, GarminError};
use crate::wellness::{self, KINDS, Kind, Period};

use super::{Report, stops_sync};

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
