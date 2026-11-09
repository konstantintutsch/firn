use chrono::{DateTime, FixedOffset};
use quick_xml::events::BytesStart;

use super::record::attribute;

/// One workout of the export.
#[derive(Debug, Clone, PartialEq)]
pub struct Workout {
    /// Apple's type without `HKWorkoutActivityType`, like `Running`.
    pub activity_type: String,
    pub start: DateTime<FixedOffset>,
    pub end: DateTime<FixedOffset>,
    pub duration_s: f64,
    pub distance_m: Option<f64>,
    pub energy_kcal: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
    /// The path of the GPX file of the route inside the export, like `/workout-routes/route.gpx`.
    pub route: Option<String>,
}

pub(super) fn read_workout(element: &BytesStart) -> Option<Workout> {
    let mut activity_type = String::new();
    let mut start = None;
    let mut end = None;
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
            b"endDate" => end = DateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S %z").ok(),
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
        end: end?,
        duration_s: duration? * seconds,
        distance_m: distance.map(|distance| distance_in_meters(distance, &distance_unit)),
        energy_kcal: energy,
        average_heart_rate: None,
        max_heart_rate: None,
        route: None,
    })
}

pub(super) fn distance_in_meters(distance: f64, unit: &str) -> f64 {
    match unit {
        "mi" => distance * 1609.344,
        "m" => distance,
        _ => distance * 1000.0,
    }
}

/// Takes what a workout's own statistics say, where the workout itself does not.
pub(super) fn read_statistics(element: &BytesStart, workout: &mut Workout) {
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
