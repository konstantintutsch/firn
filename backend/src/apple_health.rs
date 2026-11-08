//! Reads the export of Apple Health: `Export.xml` of the ZIP that the Health app
//! makes. Its records become the same daily values that Garmin's health data
//! gives, and its workouts become activities.
//!
//! The file is far too large to hold as a tree, so it is read as a stream. An
//! iPhone and an Apple Watch both record steps and the like, so each day takes
//! the values of the one source that has the most, instead of adding them up twice.

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// The kind of a health document that holds a day of values from Apple Health,
/// as `{"metrics": {"steps": 9000.0}}`.
pub const DOCUMENT_KIND: &str = "apple_health";

#[derive(Clone, Copy)]
enum Aggregate {
    /// Adds the values of a day, like steps.
    Sum,
    Mean,
    Min,
    Max,
}

struct Mapping {
    /// The record type, without `HKQuantityTypeIdentifier`.
    record: &'static str,
    metric: &'static str,
    aggregate: Aggregate,
    /// Multiplies the value, for example to turn kilometers into meters.
    scale: f64,
}

const fn map(
    record: &'static str,
    metric: &'static str,
    aggregate: Aggregate,
    scale: f64,
) -> Mapping {
    Mapping {
        record,
        metric,
        aggregate,
        scale,
    }
}

/// The part of the basal energy that only adds up to the day's total calories.
const BASAL_CALORIES: &str = "basal_calories";

const MAPPINGS: &[Mapping] = &[
    map("StepCount", "steps", Aggregate::Sum, 1.0),
    map(
        "DistanceWalkingRunning",
        "distance_m",
        Aggregate::Sum,
        1000.0,
    ),
    map("ActiveEnergyBurned", "active_calories", Aggregate::Sum, 1.0),
    map("BasalEnergyBurned", BASAL_CALORIES, Aggregate::Sum, 1.0),
    map("FlightsClimbed", "floors", Aggregate::Sum, 1.0),
    map("DietaryWater", "hydration_l", Aggregate::Sum, 0.001),
    map(
        "RestingHeartRate",
        "resting_heart_rate",
        Aggregate::Mean,
        1.0,
    ),
    map("HeartRate", "min_heart_rate", Aggregate::Min, 1.0),
    map("HeartRate", "max_heart_rate", Aggregate::Max, 1.0),
    map("HeartRateVariabilitySDNN", "hrv", Aggregate::Mean, 1.0),
    map("RespiratoryRate", "respiration", Aggregate::Mean, 1.0),
    map("OxygenSaturation", "spo2", Aggregate::Mean, 1.0),
    map("OxygenSaturation", "lowest_spo2", Aggregate::Min, 1.0),
    map("BodyMass", "weight_kg", Aggregate::Mean, 1.0),
    map(
        "BodyFatPercentage",
        "body_fat_percent",
        Aggregate::Mean,
        1.0,
    ),
];

/// One workout of the export.
#[derive(Debug, Clone, PartialEq)]
pub struct Workout {
    /// Apple's type without `HKWorkoutActivityType`, like `Running`.
    pub activity_type: String,
    pub start: DateTime<FixedOffset>,
    pub duration_s: f64,
    pub distance_m: Option<f64>,
    pub energy_kcal: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
    /// The path of the GPX file of the route inside the export, like `/workout-routes/route.gpx`.
    pub route: Option<String>,
}

#[derive(Debug, Default)]
pub struct Export {
    /// The values of each day, by Garmin's names for them.
    pub days: BTreeMap<NaiveDate, BTreeMap<String, f64>>,
    pub workouts: Vec<Workout>,
}

/// What the records of a day, from one source, add up to.
#[derive(Clone, Copy)]
struct Total {
    sum: f64,
    count: u32,
    min: f64,
    max: f64,
}

impl Total {
    fn add(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }
}

impl Default for Total {
    fn default() -> Self {
        Total {
            sum: 0.0,
            count: 0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }
}

/// Hours asleep in a night, by stage: unspecified, core, deep, REM, and awake.
type Night = [f64; 5];

/// Reads the export. Workouts are only collected if `workouts` is set.
pub fn parse(reader: impl BufRead, workouts: bool) -> Result<Export> {
    let mut xml = Reader::from_reader(reader);
    let mut buffer = Vec::new();
    let mut sources: Vec<String> = Vec::new();
    let mut source_ids: HashMap<String, usize> = HashMap::new();
    let mut totals: HashMap<(NaiveDate, usize, usize), Total> = HashMap::new();
    let mut nights: HashMap<(NaiveDate, usize), Night> = HashMap::new();
    let mut found: Vec<Workout> = Vec::new();

    let mut depth = 0usize;
    // Records inside a workout or a correlation are also listed on their own.
    let mut nested = false;
    let mut current: Option<Workout> = None;
    loop {
        let event = xml
            .read_event_into(&mut buffer)
            .context("malformed Export.xml")?;
        let (element, opens) = match &event {
            Event::Start(element) => (Some(element), true),
            Event::Empty(element) => (Some(element), false),
            _ => (None, false),
        };
        if let Some(element) = element {
            match element.local_name().as_ref() {
                b"Record" if !nested => {
                    if let Some(record) = Record::read(element) {
                        let id = *source_ids.entry(record.source.clone()).or_insert_with(|| {
                            sources.push(record.source.clone());
                            sources.len() - 1
                        });
                        record.add_to(id, &mut totals, &mut nights);
                    }
                }
                b"Workout" if depth == 1 => {
                    nested |= opens;
                    if workouts {
                        let workout = read_workout(element);
                        if opens {
                            current = workout;
                        } else {
                            found.extend(workout);
                        }
                    }
                }
                b"Correlation" => nested |= opens,
                b"WorkoutStatistics" if depth == 2 => {
                    if let Some(workout) = &mut current {
                        read_statistics(element, workout);
                    }
                }
                b"FileReference" => {
                    if let Some(workout) = &mut current {
                        workout.route = attribute(element, b"path");
                    }
                }
                _ => {}
            }
            if opens {
                depth += 1;
            }
        }
        match event {
            Event::End(element) => {
                depth = depth.saturating_sub(1);
                match element.local_name().as_ref() {
                    b"Workout" if depth == 1 => {
                        nested = false;
                        found.extend(current.take());
                    }
                    b"Correlation" => nested = false,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let mut days: BTreeMap<NaiveDate, BTreeMap<String, f64>> = BTreeMap::new();
    for (index, mapping) in MAPPINGS.iter().enumerate() {
        let mut per_day: BTreeMap<NaiveDate, Total> = BTreeMap::new();
        for (&(day, mapped, _), total) in &totals {
            if mapped != index {
                continue;
            }
            // The source with the most wins: the sum for totals, else the most values.
            let better = |other: &Total| match mapping.aggregate {
                Aggregate::Sum => total.sum > other.sum,
                _ => total.count > other.count,
            };
            if per_day.get(&day).is_none_or(better) {
                per_day.insert(day, *total);
            }
        }
        for (day, total) in per_day {
            let value = match mapping.aggregate {
                Aggregate::Sum => total.sum,
                Aggregate::Mean => total.sum / f64::from(total.count),
                Aggregate::Min => total.min,
                Aggregate::Max => total.max,
            };
            days.entry(day)
                .or_default()
                .insert(mapping.metric.to_owned(), value * mapping.scale);
        }
    }
    // The day's calories are the energy of activity and of the body at rest.
    for metrics in days.values_mut() {
        let basal = metrics.remove(BASAL_CALORIES);
        if let (Some(basal), Some(active)) = (basal, metrics.get("active_calories")) {
            let total = basal + active;
            metrics.insert("calories".to_owned(), total);
        }
    }

    let mut best_nights: BTreeMap<NaiveDate, Night> = BTreeMap::new();
    let asleep = |night: &Night| night[0] + night[1] + night[2] + night[3];
    for (&(day, _), night) in &nights {
        if best_nights
            .get(&day)
            .is_none_or(|other| asleep(night) > asleep(other))
        {
            best_nights.insert(day, *night);
        }
    }
    for (day, night) in best_nights {
        if asleep(&night) <= 0.0 {
            continue;
        }
        let metrics = days.entry(day).or_default();
        metrics.insert("sleep_h".to_owned(), asleep(&night));
        metrics.insert("light_sleep_h".to_owned(), night[1]);
        metrics.insert("deep_sleep_h".to_owned(), night[2]);
        metrics.insert("rem_sleep_h".to_owned(), night[3]);
        metrics.insert("awake_h".to_owned(), night[4]);
    }
    found.sort_by_key(|workout| workout.start);
    Ok(Export {
        days,
        workouts: found,
    })
}

fn attribute(element: &BytesStart, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
}

/// A record with the attributes that the daily values need.
struct Record {
    kind: String,
    unit: String,
    value: String,
    start: String,
    end: String,
    source: String,
}

impl Record {
    fn read(element: &BytesStart) -> Option<Record> {
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

    fn add_to(
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
fn local_time(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.get(..19)?, "%Y-%m-%d %H:%M:%S").ok()
}

fn read_workout(element: &BytesStart) -> Option<Workout> {
    let mut activity_type = String::new();
    let mut start = None;
    let mut duration = None;
    let mut duration_unit = String::new();
    let mut distance = None;
    let mut distance_unit = String::new();
    let mut energy = None;
    for attribute in element.attributes().flatten() {
        let value = String::from_utf8_lossy(&attribute.value).into_owned();
        match attribute.key.as_ref() {
            b"workoutActivityType" => {
                activity_type = value
                    .strip_prefix("HKWorkoutActivityType")
                    .unwrap_or(&value)
                    .to_owned();
            }
            b"startDate" => start = DateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S %z").ok(),
            b"duration" => duration = value.parse::<f64>().ok(),
            b"durationUnit" => duration_unit = value,
            b"totalDistance" => distance = value.parse::<f64>().ok(),
            b"totalDistanceUnit" => distance_unit = value,
            b"totalEnergyBurned" => energy = value.parse::<f64>().ok(),
            _ => {}
        }
    }
    let seconds = match duration_unit.as_str() {
        "sec" | "s" => 1.0,
        "hr" => 3600.0,
        _ => 60.0,
    };
    Some(Workout {
        activity_type,
        start: start?,
        duration_s: duration? * seconds,
        distance_m: distance.map(|distance| distance_in_meters(distance, &distance_unit)),
        energy_kcal: energy,
        average_heart_rate: None,
        max_heart_rate: None,
        route: None,
    })
}

fn distance_in_meters(distance: f64, unit: &str) -> f64 {
    match unit {
        "mi" => distance * 1609.344,
        "m" => distance,
        _ => distance * 1000.0,
    }
}

/// Takes what a workout's own statistics say, where the workout itself does not.
fn read_statistics(element: &BytesStart, workout: &mut Workout) {
    let number = |name: &[u8]| attribute(element, name).and_then(|value| value.parse::<f64>().ok());
    let unit = attribute(element, b"unit").unwrap_or_default();
    match attribute(element, b"type")
        .as_deref()
        .and_then(|kind| kind.strip_prefix("HKQuantityTypeIdentifier"))
    {
        Some("HeartRate") => {
            workout.average_heart_rate = number(b"average");
            workout.max_heart_rate = number(b"maximum");
        }
        Some("ActiveEnergyBurned") if workout.energy_kcal.is_none() => {
            workout.energy_kcal = number(b"sum");
        }
        Some("DistanceWalkingRunning" | "DistanceCycling") if workout.distance_m.is_none() => {
            workout.distance_m = number(b"sum").map(|sum| distance_in_meters(sum, &unit));
        }
        _ => {}
    }
}

/// Garmin's type key for an Apple workout type, and a name to give the activity.
pub fn activity_type(apple: &str) -> (String, String) {
    let (key, name) = match apple {
        "Running" => ("running", "Running"),
        "Walking" => ("walking", "Walking"),
        "Hiking" => ("hiking", "Hiking"),
        "Cycling" => ("cycling", "Cycling"),
        "TraditionalStrengthTraining" | "FunctionalStrengthTraining" => {
            ("strength_training", "Strength training")
        }
        _ => {
            // `CoreTraining` becomes `core_training`.
            let mut key = String::new();
            for (index, letter) in apple.chars().enumerate() {
                if letter.is_uppercase() && index > 0 {
                    key.push('_');
                }
                key.extend(letter.to_lowercase());
            }
            let key = if key.is_empty() {
                "other".to_owned()
            } else {
                key
            };
            let mut name = key.replace('_', " ");
            if let Some(first) = name.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            return (key, name);
        }
    };
    (key.to_owned(), name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE HealthData [
<!ELEMENT HealthData (ExportDate,Me,(Record|Workout)*)>
<!ATTLIST HealthData locale CDATA #REQUIRED>
]>
<HealthData locale="en_DE">
 <ExportDate value="2026-04-12 08:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierStepCount" unit="count" value="1000" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 09:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierStepCount" unit="count" value="500" sourceName="Watch" startDate="2026-04-10 10:00:00 +0200" endDate="2026-04-10 11:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierStepCount" unit="count" value="1200" sourceName="Phone" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 09:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min" value="60" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 08:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min" value="150" sourceName="Watch" startDate="2026-04-10 08:30:00 +0200" endDate="2026-04-10 08:30:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierOxygenSaturation" unit="%" value="0.97" sourceName="Watch" startDate="2026-04-10 08:30:00 +0200" endDate="2026-04-10 08:30:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierActiveEnergyBurned" unit="kcal" value="300" sourceName="Watch" startDate="2026-04-10 08:30:00 +0200" endDate="2026-04-10 08:30:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierBasalEnergyBurned" unit="kcal" value="1700" sourceName="Watch" startDate="2026-04-10 08:30:00 +0200" endDate="2026-04-10 08:30:00 +0200"/>
 <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisAsleepCore" sourceName="Watch" startDate="2026-04-09 23:00:00 +0200" endDate="2026-04-10 02:00:00 +0200"/>
 <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisAsleepDeep" sourceName="Watch" startDate="2026-04-10 02:00:00 +0200" endDate="2026-04-10 03:00:00 +0200"/>
 <Record type="HKCategoryTypeIdentifierSleepAnalysis" value="HKCategoryValueSleepAnalysisInBed" sourceName="Phone" startDate="2026-04-09 22:00:00 +0200" endDate="2026-04-10 07:00:00 +0200"/>
 <Workout workoutActivityType="HKWorkoutActivityTypeRunning" duration="30" durationUnit="min" totalDistance="5" totalDistanceUnit="km" totalEnergyBurned="350" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 08:30:00 +0200">
  <Record type="HKQuantityTypeIdentifierStepCount" unit="count" value="9999" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 08:30:00 +0200"/>
  <WorkoutStatistics type="HKQuantityTypeIdentifierHeartRate" average="150" minimum="100" maximum="170" unit="count/min"/>
  <WorkoutActivity>
   <WorkoutStatistics type="HKQuantityTypeIdentifierHeartRate" average="1" minimum="1" maximum="1" unit="count/min"/>
  </WorkoutActivity>
  <WorkoutRoute sourceName="Watch"><FileReference path="/workout-routes/route.gpx"/></WorkoutRoute>
 </Workout>
</HealthData>"#;

    fn day() -> NaiveDate {
        "2026-04-10".parse().unwrap()
    }

    #[test]
    fn days_take_the_source_with_the_most_and_skip_records_inside_workouts() {
        let export = parse(EXPORT.as_bytes(), false).unwrap();
        let metrics = &export.days[&day()];
        // The watch's 1500 steps, not the phone's 1200, and not added to them.
        assert_eq!(metrics["steps"], 1500.0);
        assert_eq!(metrics["min_heart_rate"], 60.0);
        assert_eq!(metrics["max_heart_rate"], 150.0);
        assert!((metrics["spo2"] - 97.0).abs() < 1e-9);
        assert_eq!(metrics["active_calories"], 300.0);
        assert_eq!(metrics["calories"], 2000.0);
        assert!(!metrics.contains_key(BASAL_CALORIES));
        assert!(export.workouts.is_empty());
    }

    #[test]
    fn nights_belong_to_the_day_they_end_on() {
        let metrics = &parse(EXPORT.as_bytes(), false).unwrap().days[&day()];
        assert_eq!(metrics["light_sleep_h"], 3.0);
        assert_eq!(metrics["deep_sleep_h"], 1.0);
        assert_eq!(metrics["sleep_h"], 4.0);
        assert!(!export_has_day(
            &parse(EXPORT.as_bytes(), false).unwrap(),
            "2026-04-09"
        ));
    }

    fn export_has_day(export: &Export, day: &str) -> bool {
        export.days.contains_key(&day.parse::<NaiveDate>().unwrap())
    }

    #[test]
    fn workouts_come_with_their_own_statistics_and_route() {
        let export = parse(EXPORT.as_bytes(), true).unwrap();
        let [workout] = &export.workouts[..] else {
            panic!("{:?}", export.workouts);
        };
        assert_eq!(workout.activity_type, "Running");
        assert_eq!(workout.duration_s, 1800.0);
        assert_eq!(workout.distance_m, Some(5000.0));
        assert_eq!(workout.energy_kcal, Some(350.0));
        assert_eq!(workout.average_heart_rate, Some(150.0));
        assert_eq!(workout.max_heart_rate, Some(170.0));
        assert_eq!(workout.route.as_deref(), Some("/workout-routes/route.gpx"));
        assert_eq!(workout.start.to_rfc3339(), "2026-04-10T08:00:00+02:00");
        // Its records stay out of the days.
        assert_eq!(export.days[&day()]["steps"], 1500.0);
    }

    #[test]
    fn workout_types_become_garmins() {
        assert_eq!(activity_type("Running").0, "running");
        assert_eq!(
            activity_type("FunctionalStrengthTraining").0,
            "strength_training"
        );
        assert_eq!(
            activity_type("CoreTraining"),
            ("core_training".to_owned(), "Core training".to_owned())
        );
    }
}
