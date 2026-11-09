use chrono::NaiveDate;
use serde_json::Value;

use super::{Dates, Kind, Metric};

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
