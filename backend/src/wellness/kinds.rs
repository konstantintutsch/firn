use super::{Dates, Kind, Metric, Period};

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
