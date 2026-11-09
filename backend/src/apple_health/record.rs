use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime};
use quick_xml::events::BytesStart;

use super::mappings::MAPPINGS;
use super::{Night, Total};

/// One VO2 max that the Health app measured, usually after a workout.
#[derive(Debug, Clone, PartialEq)]
pub struct Vo2Max {
    pub time: DateTime<FixedOffset>,
    /// In mL per minute and kg.
    pub value: f64,
}

pub(super) fn attribute(element: &BytesStart, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
}

/// A record with the attributes that the daily values need.
pub(super) struct Record {
    kind: String,
    unit: String,
    value: String,
    start: String,
    end: String,
    pub(super) source: String,
}

impl Record {
    pub(super) fn read(element: &BytesStart) -> Option<Record> {
        let mut record = Record {
            kind: String::new(),
            unit: String::new(),
            value: String::new(),
            start: String::new(),
            end: String::new(),
            source: String::new(),
        };
        for attribute in element.attributes().flatten() {
            let slot = match attribute.key.as_ref() {
                b"type" => &mut record.kind,
                b"unit" => &mut record.unit,
                b"value" => &mut record.value,
                b"startDate" => &mut record.start,
                b"endDate" => &mut record.end,
                b"sourceName" => &mut record.source,
                _ => continue,
            };
            *slot = String::from_utf8_lossy(&attribute.value).into_owned();
        }
        (!record.kind.is_empty() && !record.start.is_empty()).then_some(record)
    }

    /// The VO2 max of this record, if it is one.
    pub(super) fn vo2_max(&self) -> Option<Vo2Max> {
        if self.kind != "HKQuantityTypeIdentifierVO2Max" {
            return None;
        }
        let value = self
            .value
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)?;
        let time = DateTime::parse_from_str(&self.start, "%Y-%m-%d %H:%M:%S %z").ok()?;
        Some(Vo2Max { time, value })
    }

    pub(super) fn add_to(
        &self,
        source: usize,
        totals: &mut HashMap<(NaiveDate, usize, usize), Total>,
        nights: &mut HashMap<(NaiveDate, usize), Night>,
    ) {
        if self.kind == "HKCategoryTypeIdentifierSleepAnalysis" {
            self.add_sleep(source, nights);
            return;
        }
        let Some(kind) = self.kind.strip_prefix("HKQuantityTypeIdentifier") else {
            return;
        };
        let Some(mut value) = self.value.parse::<f64>().ok().filter(|v| v.is_finite()) else {
            return;
        };
        // Percentages are fractions of one, like 0.97 for 97 %.
        if self.unit == "%" && value <= 1.0 {
            value *= 100.0;
        }
        // Distances and energy come in the units that the phone is set to.
        value = match self.unit.as_str() {
            "mi" => value * 1.609_344,
            "cal" => value / 1000.0,
            "kJ" => value / 4.184,
            "lb" => value * 0.453_592_37,
            "degF" => (value - 32.0) / 1.8,
            _ => value,
        };
        let Some(day) = self
            .start
            .get(..10)
            .and_then(|day| day.parse::<NaiveDate>().ok())
        else {
            return;
        };
        for (index, mapping) in MAPPINGS.iter().enumerate() {
            if mapping.record == kind {
                totals.entry((day, index, source)).or_default().add(value);
            }
        }
    }

    fn add_sleep(&self, source: usize, nights: &mut HashMap<(NaiveDate, usize), Night>) {
        let (Some(start), Some(end)) = (local_time(&self.start), local_time(&self.end)) else {
            return;
        };
        let stage = match self
            .value
            .strip_prefix("HKCategoryValueSleepAnalysis")
            .unwrap_or_default()
        {
            "AsleepUnspecified" | "Asleep" => 0,
            "AsleepCore" => 1,
            "AsleepDeep" => 2,
            "AsleepREM" => 3,
            "Awake" => 4,
            // Time in bed says nothing about sleep.
            _ => return,
        };
        let hours = (end - start).num_seconds().max(0) as f64 / 3600.0;
        // A night belongs to the day it ends on, which twelve hours after its start still is.
        let night = (start + Duration::hours(12)).date();
        nights.entry((night, source)).or_default()[stage] += hours;
    }
}

/// The wall clock time of a date like `2026-04-11 07:12:11 +0200`, without its offset.
pub(super) fn local_time(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.get(..19)?, "%Y-%m-%d %H:%M:%S").ok()
}
