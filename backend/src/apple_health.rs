//! Reads the export of Apple Health: `Export.xml` of the ZIP that the Health app
//! makes. Its records become the same daily values that Garmin's health data
//! gives, and its workouts become activities.
//!
//! The file is far too large to hold as a tree, so it is read as a stream. An
//! iPhone and an Apple Watch both record steps and the like, so each day takes
//! the values of the one source that has the most, instead of adding them up twice.

mod mappings;
mod record;
mod workout;

pub use record::Vo2Max;
pub use workout::{Workout, activity_type};

use mappings::{Aggregate, BASAL_CALORIES, HRV_AVERAGE_DAYS, MAPPINGS};
use record::{Record, attribute};
use workout::{read_statistics, read_workout};

use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate};
use quick_xml::Reader;
use quick_xml::events::Event;

/// The kind of a health document that holds a day of values from Apple Health,
/// as `{"metrics": {"steps": 9000.0}}`.
pub const DOCUMENT_KIND: &str = "apple_health";

#[derive(Debug, Default)]
pub struct Export {
    /// Every VO2 max, oldest first. None is dropped, even if a day has several.
    pub vo2_max: Vec<Vo2Max>,
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
    let mut vo2_max: Vec<Vo2Max> = Vec::new();

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
                        if let Some(measured) = record.vo2_max() {
                            vo2_max.push(measured);
                        }
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

    // Garmin reports an average of the last seven days next to the night's HRV. Apple does not.
    let hrv: BTreeMap<NaiveDate, f64> = days
        .iter()
        .filter_map(|(day, metrics)| Some((*day, *metrics.get("hrv")?)))
        .collect();
    for &day in hrv.keys() {
        let from = day - Duration::days(HRV_AVERAGE_DAYS - 1);
        let window: Vec<f64> = hrv.range(from..=day).map(|(_, value)| *value).collect();
        let average = window.iter().sum::<f64>() / window.len() as f64;
        days.entry(day)
            .or_default()
            .insert("hrv_weekly".to_owned(), average);
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
    vo2_max.sort_by_key(|measured| measured.time);
    Ok(Export {
        vo2_max,
        days,
        workouts: found,
    })
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
 <Record type="HKQuantityTypeIdentifierVO2Max" unit="mL/min·kg" value="49.5" sourceName="Watch" startDate="2026-04-10 08:31:00 +0200" endDate="2026-04-10 08:31:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierVO2Max" unit="mL/min·kg" value="49.7" sourceName="Watch" startDate="2026-04-10 19:00:00 +0200" endDate="2026-04-10 19:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierDietaryWater" unit="mL" value="500" sourceName="Phone" startDate="2026-04-10 12:00:00 +0200" endDate="2026-04-10 12:00:00 +0200"/>
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
    fn every_vo2_max_is_kept_and_water_is_not() {
        let export = parse(EXPORT.as_bytes(), false).unwrap();
        let values: Vec<f64> = export
            .vo2_max
            .iter()
            .map(|measured| measured.value)
            .collect();
        assert_eq!(values, [49.5, 49.7]);
        assert_eq!(
            export.vo2_max[0].time.to_rfc3339(),
            "2026-04-10T08:31:00+02:00"
        );
        let metrics = &export.days[&day()];
        assert!(!metrics.contains_key("vo2_max") && !metrics.contains_key("hydration_l"));
    }

    #[test]
    fn hrv_gets_an_average_of_the_last_seven_days() {
        let record = |day: u32, value: u32| {
            format!(
                r#"<Record type="HKQuantityTypeIdentifierHeartRateVariabilitySDNN" unit="ms" value="{value}" sourceName="Watch" startDate="2026-04-{day:02} 03:00:00 +0200" endDate="2026-04-{day:02} 03:00:00 +0200"/>"#
            )
        };
        // Days 1, 2 and 9, so that day 9 no longer reaches back to day 1 and 2.
        let xml = format!(
            "<HealthData>{}{}{}</HealthData>",
            record(1, 40),
            record(2, 50),
            record(9, 70)
        );
        let days = parse(xml.as_bytes(), false).unwrap().days;
        let weekly = |day: &str| days[&day.parse::<NaiveDate>().unwrap()]["hrv_weekly"];
        assert_eq!(weekly("2026-04-01"), 40.0);
        assert_eq!(weekly("2026-04-02"), 45.0);
        assert_eq!(weekly("2026-04-09"), 70.0);
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
        assert_eq!(workout.end.to_rfc3339(), "2026-04-10T08:30:00+02:00");
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
