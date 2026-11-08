//! Copies the health data of the Garmin Personal API, like sleep, HRV and stress.
//!
//! Every response is saved as it came, and the daily values that the statistics
//! show are read from it. Garmin does not document the response fields, so each
//! metric lists the places where Garmin Connect is known to put it.

use chrono::{Days, NaiveDate};
use serde_json::Value;

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

const fn metric(name: &'static str, paths: &'static [&'static str]) -> Metric {
    Metric {
        name,
        paths,
        scale: 1.0,
    }
}

const fn scaled(name: &'static str, paths: &'static [&'static str], scale: f64) -> Metric {
    Metric { name, paths, scale }
}

pub const KINDS: &[Kind] = &[
    Kind {
        name: "daily_summaries",
        period: Period::Range,
        path: "/usersummary-service/v2/dailysummaries",
        dates: Dates::RangeQuery,
        // The v2 summaries nest their values, like `movement.steps.value`. The flat
        // names of the older summaries come first, so that a moved key is still found.
        metrics: &[
            metric("steps", &["totalSteps", "movement.steps.value"]),
            metric(
                "step_goal",
                &["dailyStepGoal", "stepGoal", "movement.steps.goal"],
            ),
            metric(
                "distance_m",
                &[
                    "totalDistanceMeters",
                    "totalDistance",
                    "movement.steps.distanceInMeters",
                ],
            ),
            metric(
                "resting_heart_rate",
                &["restingHeartRate", "heartRate.restingValue"],
            ),
            metric("min_heart_rate", &["minHeartRate", "heartRate.minValue"]),
            metric("max_heart_rate", &["maxHeartRate", "heartRate.maxValue"]),
            metric(
                "stress",
                &[
                    "averageStressLevel",
                    "allDayStress.averageStressLevel",
                    "stress.avgLevel",
                ],
            ),
            metric("calories", &["totalKilocalories", "calories.burnedTotal"]),
            metric(
                "active_calories",
                &["activeKilocalories", "calories.burnedActive"],
            ),
            metric(
                "floors",
                &["floorsAscended", "movement.floorsAscended.value"],
            ),
            metric(
                "floors_descended",
                &["floorsDescended", "movement.floorsDescended.value"],
            ),
            metric(
                "moderate_intensity_minutes",
                &["moderateIntensityMinutes", "intensityMinutes.moderate"],
            ),
            metric(
                "vigorous_intensity_minutes",
                &["vigorousIntensityMinutes", "intensityMinutes.vigorous"],
            ),
            metric(
                "body_battery_high",
                &["bodyBatteryHighestValue", "bodyBattery.maxValue"],
            ),
            metric(
                "body_battery_low",
                &["bodyBatteryLowestValue", "bodyBattery.minValue"],
            ),
            metric(
                "body_battery_charged",
                &["bodyBatteryChargedValue", "bodyBattery.chargedValue"],
            ),
            metric(
                "body_battery_drained",
                &["bodyBatteryDrainedValue", "bodyBattery.drainedValue"],
            ),
            metric("spo2", &["averageSpo2", "pulseOx.avgValue"]),
            metric("lowest_spo2", &["lowestSpo2", "pulseOx.minValue"]),
            metric(
                "respiration",
                &["avgWakingRespirationValue", "respiration.avgValue"],
            ),
        ],
    },
    Kind {
        name: "sleep_summaries",
        period: Period::Range,
        path: "/sleep-service/sleep/dailySleepSummariesByDate",
        dates: Dates::RangeQuery,
        metrics: SLEEP_METRICS,
    },
    Kind {
        name: "hrv_summaries",
        period: Period::Range,
        path: "/hrv-service/hrv/daily/{start}/{end}",
        dates: Dates::Path,
        metrics: HRV_METRICS,
    },
    Kind {
        name: "body_battery",
        period: Period::Range,
        path: "/wellness-service/wellness/bodyBattery/reports/daily",
        dates: Dates::RangeQuery,
        metrics: &[
            metric("body_battery_charged", &["charged"]),
            metric("body_battery_drained", &["drained"]),
        ],
    },
    Kind {
        name: "stress",
        period: Period::Range,
        path: "/usersummary-service/stats/stress/daily/{start}/{end}",
        dates: Dates::Path,
        metrics: &[metric(
            "stress",
            &["values.overallStressLevel", "overallStressLevel"],
        )],
    },
    Kind {
        name: "steps",
        period: Period::Range,
        path: "/usersummary-service/stats/steps/daily/{start}/{end}",
        dates: Dates::Path,
        metrics: &[
            metric("steps", &["totalSteps", "values.totalSteps"]),
            metric("step_goal", &["stepGoal", "values.stepGoal"]),
        ],
    },
    Kind {
        name: "hydration",
        period: Period::Range,
        path: "/usersummary-service/stats/hydration/daily/{start}/{end}",
        dates: Dates::Path,
        metrics: &[
            scaled("hydration_l", &["values.valueInML", "valueInML"], 0.001),
            scaled("hydration_goal_l", &["values.goalInML", "goalInML"], 0.001),
        ],
    },
    Kind {
        name: "weight",
        period: Period::Range,
        path: "/weight-service/weight/range/{start}/{end}",
        dates: Dates::Path,
        metrics: &[
            scaled("weight_kg", &["latestWeight.weight", "weight"], 0.001),
            metric("bmi", &["latestWeight.bmi", "bmi"]),
            metric("body_fat_percent", &["latestWeight.bodyFat", "bodyFat"]),
            scaled(
                "muscle_mass_kg",
                &["latestWeight.muscleMass", "muscleMass"],
                0.001,
            ),
        ],
    },
    Kind {
        name: "weight_goals",
        period: Period::Range,
        path: "/goal-service/goal/user/effective/weightgoal/{start}/{end}",
        dates: Dates::Path,
        metrics: &[],
    },
    Kind {
        name: "wellness_activities",
        period: Period::Range,
        path: "/wellnessactivity-service/activity/summary/range/{start}/{end}",
        dates: Dates::Path,
        metrics: &[],
    },
    Kind {
        name: "sleep",
        period: Period::Day,
        path: "/sleep-service/sleep/dailySleep",
        dates: Dates::DayQuery("date"),
        metrics: SLEEP_METRICS,
    },
    Kind {
        name: "hrv",
        period: Period::Day,
        path: "/hrv-service/hrv/{date}",
        dates: Dates::Path,
        metrics: HRV_METRICS,
    },
    Kind {
        name: "heart_rate",
        period: Period::Day,
        path: "/wellness-service/wellness/dailyHeartRate",
        dates: Dates::DayQuery("date"),
        metrics: &[
            metric("resting_heart_rate", &["restingHeartRate"]),
            metric("min_heart_rate", &["minHeartRate"]),
            metric("max_heart_rate", &["maxHeartRate"]),
        ],
    },
    Kind {
        name: "spo2",
        period: Period::Day,
        path: "/wellness-service/wellness/daily/spo2acclimation/{date}",
        dates: Dates::Path,
        metrics: &[
            metric("spo2", &["averageSpO2", "averageSpo2"]),
            metric("lowest_spo2", &["lowestSpO2", "lowestSpo2"]),
        ],
    },
    Kind {
        name: "food_logs",
        period: Period::Day,
        path: "/nutrition-service/food/logs/{date}",
        dates: Dates::Path,
        metrics: &[],
    },
    Kind {
        name: "menstrual_cycle",
        period: Period::Day,
        path: "/periodichealth-service/menstrualcycle/summary/{date}",
        dates: Dates::Path,
        metrics: &[],
    },
    Kind {
        name: "fitness_age",
        period: Period::Today,
        path: "/fitnessage-service/fitnessage/v2/{date}",
        dates: Dates::Path,
        metrics: &[
            metric("fitness_age", &["fitnessAge"]),
            metric("chronological_age", &["chronologicalAge"]),
        ],
    },
    Kind {
        name: "profile_settings",
        period: Period::Undated,
        path: "/userprofile-service/userprofile/settings",
        dates: Dates::None,
        metrics: &[],
    },
    Kind {
        name: "heart_rate_zones",
        period: Period::Undated,
        path: "/biometric-service/heartRateZones/v2",
        dates: Dates::None,
        metrics: &[],
    },
];

const SLEEP_METRICS: &[Metric] = &[
    scaled("sleep_h", &["sleepTimeSeconds"], 1.0 / 3600.0),
    scaled("deep_sleep_h", &["deepSleepSeconds"], 1.0 / 3600.0),
    scaled("light_sleep_h", &["lightSleepSeconds"], 1.0 / 3600.0),
    scaled("rem_sleep_h", &["remSleepSeconds"], 1.0 / 3600.0),
    scaled("awake_h", &["awakeSleepSeconds"], 1.0 / 3600.0),
    metric(
        "sleep_score",
        &[
            "sleepScores.overall.value",
            "overallSleepScore.value",
            "sleepScore",
        ],
    ),
    metric(
        "sleep_respiration",
        &["averageRespirationValue", "avgSleepRespirationValue"],
    ),
    metric("sleep_stress", &["avgSleepStress"]),
];

const HRV_METRICS: &[Metric] = &[
    metric("hrv", &["lastNightAvg"]),
    metric("hrv_weekly", &["weeklyAvg"]),
    metric("hrv_high", &["lastNight5MinHigh"]),
    metric("hrv_baseline_low", &["baseline.balancedLow"]),
    metric("hrv_baseline_high", &["baseline.balancedUpper"]),
];

/// Keys that hold the day a record belongs to.
const DATE_KEYS: &[&str] = &["calendarDate", "summaryDate", "date"];

impl Kind {
    pub fn request(&self, period: &str) -> (String, Vec<(&'static str, String)>) {
        let (start, end) = period.split_once('/').unwrap_or((period, period));
        match self.dates {
            Dates::None => (self.path.to_owned(), Vec::new()),
            Dates::Path => (
                self.path
                    .replace("{date}", start)
                    .replace("{start}", start)
                    .replace("{end}", end),
                Vec::new(),
            ),
            Dates::DayQuery(name) => (self.path.to_owned(), vec![(name, start.to_owned())]),
            Dates::RangeQuery => (
                self.path.to_owned(),
                vec![("startDate", start.to_owned()), ("endDate", end.to_owned())],
            ),
        }
    }

    /// The daily values in a response. `day` is used for records without a date.
    pub fn extract(
        &self,
        data: &Value,
        day: Option<NaiveDate>,
    ) -> Vec<(NaiveDate, &'static str, f64)> {
        if self.metrics.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let mut records = Vec::new();
        collect_records(data, &mut records);
        if records.is_empty()
            && let Some(day) = day
        {
            records.push((day, data));
        }
        for (day, record) in records {
            for metric in self.metrics {
                if let Some(value) = metric.read(record) {
                    found.push((day, metric.name, value));
                }
            }
        }
        found
    }
}

impl Metric {
    fn read(&self, record: &Value) -> Option<f64> {
        let value = self
            .paths
            .iter()
            .find_map(|path| number(at_path(record, path)?))
            .or_else(|| {
                let key = self.paths.first()?.rsplit('.').next()?;
                find_key(record, key)
            })?;
        Some(value * self.scale)
    }
}

fn at_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |value, key| value.get(key))
}

/// Garmin sends some numbers as strings.
fn number(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

fn find_key(value: &Value, key: &str) -> Option<f64> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(number)
            .or_else(|| object.values().find_map(|value| find_key(value, key))),
        Value::Array(items) => items.iter().find_map(|value| find_key(value, key)),
        _ => None,
    }
}

fn record_day(value: &Value) -> Option<NaiveDate> {
    DATE_KEYS.iter().find_map(|key| {
        let text = value.get(key)?.as_str()?;
        NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
    })
}

/// Finds the outermost objects with a date. Their contents belong to that day.
fn collect_records<'a>(value: &'a Value, records: &mut Vec<(NaiveDate, &'a Value)>) {
    if let Some(day) = record_day(value) {
        records.push((day, value));
        return;
    }
    match value {
        Value::Object(object) => object
            .values()
            .for_each(|value| collect_records(value, records)),
        Value::Array(items) => items
            .iter()
            .for_each(|value| collect_records(value, records)),
        _ => {}
    }
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
