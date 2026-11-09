#[derive(Clone, Copy)]
pub(super) enum Aggregate {
    /// Adds the values of a day, like steps.
    Sum,
    Mean,
    Min,
    Max,
}

pub(super) struct Mapping {
    /// The record type, without `HKQuantityTypeIdentifier`.
    pub(super) record: &'static str,
    pub(super) metric: &'static str,
    pub(super) aggregate: Aggregate,
    /// Multiplies the value, for example to turn kilometers into meters.
    pub(super) scale: f64,
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

/// Days in the average of the HRV: the day itself and the ones before.
pub(super) const HRV_AVERAGE_DAYS: i64 = 7;

/// The part of the basal energy that only adds up to the day's total calories.
pub(super) const BASAL_CALORIES: &str = "basal_calories";

pub(super) const MAPPINGS: &[Mapping] = &[
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
