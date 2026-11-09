//! Copies the health data of the Garmin Personal API, like sleep, HRV and stress.
//!
//! Every response is saved as it came, and the daily values that the statistics
//! show are read from it. Garmin does not document the response fields, so each
//! metric lists the places where Garmin Connect is known to put it.
mod extract;
mod kinds;

pub use kinds::KINDS;

use chrono::{Days, NaiveDate};

/// Ranges this long stay within the limits of all of Garmin's range endpoints.
pub const RANGE_DAYS: u64 = 28;
/// Garmin keeps updating recent days, for example when a watch syncs the night
/// later, so these are fetched again on every sync.
pub const REFRESH_DAYS: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Period {
    /// One request per day.
    Day,
    /// One request per range of `RANGE_DAYS` days.
    Range,
    /// One request per sync, saved under the day of the sync.
    Today,
    /// One request per sync, for data without a date.
    Undated,
}

pub struct Kind {
    pub name: &'static str,
    pub period: Period,
    pub path: &'static str,
    /// How the dates go into the request.
    pub dates: Dates,
    pub metrics: &'static [Metric],
}

#[derive(Debug, Clone, Copy)]
pub enum Dates {
    None,
    /// `{date}` in the path is replaced by the day, `{start}` and `{end}` by the range.
    Path,
    /// A query parameter with this name holds the day.
    DayQuery(&'static str),
    /// `startDate` and `endDate` query parameters.
    RangeQuery,
}

pub struct Metric {
    pub name: &'static str,
    /// Dotted paths from a day's record, tried in order. The last part of the
    /// first is also searched anywhere in the record as a last resort.
    pub paths: &'static [&'static str],
    /// Multiplies the value, for example to turn grams into kilograms.
    pub scale: f64,
}

/// The fixed ranges that cover the days from `first` to `last`. Fixed, so that a
/// range saved by one sync is found again by the next.
pub fn ranges(first: NaiveDate, last: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 3).expect("valid date");
    let offset = (first - epoch).num_days().rem_euclid(RANGE_DAYS as i64) as u64;
    let mut start = first - Days::new(offset);
    let mut ranges = Vec::new();
    while start <= last {
        let end = start + Days::new(RANGE_DAYS - 1);
        ranges.push((start, end));
        start = end + Days::new(1);
    }
    ranges
}

pub fn range_period(start: NaiveDate, end: NaiveDate) -> String {
    format!("{start}/{end}")
}

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn day(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    #[test]
    fn ranges_are_fixed_and_cover_all_days() {
        let all = ranges(day("2026-01-01"), day("2026-03-01"));
        assert!(all.first().unwrap().0 <= day("2026-01-01"));
        assert!(all.last().unwrap().1 >= day("2026-03-01"));
        for pair in all.windows(2) {
            assert_eq!(pair[0].1 + Days::new(1), pair[1].0);
        }
        // A later first day still finds the same ranges.
        let later = ranges(day("2026-01-20"), day("2026-03-01"));
        assert_eq!(later, all[all.len() - later.len()..]);
    }

    #[test]
    fn builds_requests_for_each_kind_of_dates() {
        let hrv = kind("hrv_summaries").unwrap();
        assert_eq!(
            hrv.request("2026-01-01/2026-01-28"),
            (
                "/hrv-service/hrv/daily/2026-01-01/2026-01-28".to_owned(),
                vec![]
            )
        );
        let sleep = kind("sleep").unwrap();
        assert_eq!(
            sleep.request("2026-01-01"),
            (
                "/sleep-service/sleep/dailySleep".to_owned(),
                vec![("date", "2026-01-01".to_owned())]
            )
        );
        let summaries = kind("daily_summaries").unwrap();
        assert_eq!(
            summaries.request("2026-01-01/2026-01-28").1,
            vec![
                ("startDate", "2026-01-01".to_owned()),
                ("endDate", "2026-01-28".to_owned())
            ]
        );
    }

    #[test]
    fn reads_each_day_of_a_range() {
        let data = json!({"hrvSummaries": [
            {"calendarDate": "2026-01-01", "lastNightAvg": 41, "weeklyAvg": 44,
             "baseline": {"balancedLow": 38, "balancedUpper": 50}},
            {"calendarDate": "2026-01-02", "lastNightAvg": "45"},
        ]});
        let found = kind("hrv_summaries").unwrap().extract(&data, None);
        assert!(found.contains(&(day("2026-01-01"), "hrv", 41.0)));
        assert!(found.contains(&(day("2026-01-01"), "hrv_baseline_high", 50.0)));
        assert!(found.contains(&(day("2026-01-02"), "hrv", 45.0)));
        assert!(!found.contains(&(day("2026-01-02"), "hrv_weekly", 44.0)));
    }

    #[test]
    fn reads_nested_records_and_scales() {
        let data = json!({"dailySleepDTO": {
            "calendarDate": "2026-01-01",
            "sleepTimeSeconds": 27000,
            "deepSleepSeconds": 3600,
            "sleepScores": {"overall": {"value": 82}},
            "sleepLevels": [{"startGMT": "2026-01-01T00:00:00.0"}],
        }});
        let found = kind("sleep").unwrap().extract(&data, None);
        assert!(found.contains(&(day("2026-01-01"), "sleep_h", 7.5)));
        assert!(found.contains(&(day("2026-01-01"), "deep_sleep_h", 1.0)));
        assert!(found.contains(&(day("2026-01-01"), "sleep_score", 82.0)));
    }

    #[test]
    fn searches_for_a_key_that_moved() {
        let data =
            json!({"calendarDate": "2026-01-01", "stats": {"wellness": {"totalSteps": 9000}}});
        let found = kind("daily_summaries").unwrap().extract(&data, None);
        assert_eq!(found, [(day("2026-01-01"), "steps", 9000.0)]);
    }

    #[test]
    fn reads_the_nested_values_of_v2_summaries() {
        let data = json!({"data": [{
            "calendarDate": "2026-11-01",
            "movement": {
                "steps": {"goal": 10000, "value": 6799, "distanceInMeters": 7604.0},
                "floorsAscended": {"goal": 10, "value": 10, "distanceInMeters": 32.22},
                "floorsDescended": {"value": 11, "distanceInMeters": 35.81},
            },
            "heartRate": {"minValue": 48, "maxValue": 214, "restingValue": 52},
            "bodyBattery": {"minValue": 20, "maxValue": 77, "chargedValue": 70, "drainedValue": 49},
        }]});
        let found = kind("daily_summaries").unwrap().extract(&data, None);
        let day = day("2026-11-01");
        for expected in [
            (day, "steps", 6799.0),
            (day, "step_goal", 10000.0),
            (day, "floors", 10.0),
            (day, "floors_descended", 11.0),
            (day, "resting_heart_rate", 52.0),
            (day, "body_battery_high", 77.0),
            (day, "body_battery_low", 20.0),
        ] {
            assert!(found.contains(&expected), "missing {expected:?}");
        }
    }

    #[test]
    fn uses_the_requested_day_for_records_without_one() {
        let data = json!({"fitnessAge": 31.5, "chronologicalAge": 35});
        let found = kind("fitness_age")
            .unwrap()
            .extract(&data, Some(day("2026-01-01")));
        assert_eq!(
            found,
            [
                (day("2026-01-01"), "fitness_age", 31.5),
                (day("2026-01-01"), "chronological_age", 35.0)
            ]
        );
    }
}
