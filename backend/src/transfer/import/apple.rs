use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, Read};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};
use uuid::Uuid;
use zip::ZipArchive;

use super::files::summary_of;
use super::{AppleHealthScope, Import, ImportReport};
use crate::apple_health;
use crate::database;
use crate::garmin::Activity;
use crate::track::{self, Sample};
use crate::transfer::entries::{MAX_ENTRY_BYTES, imported_id, read_entry, single_file_zip};

/// How long after a workout ends its VO2 max can still be measured and count for it.
const VO2_MAX_AFTER_WORKOUT_MINUTES: i64 = 30;

impl Import<'_> {
    /// Imports the health values of an Apple Health export, and with
    /// `AppleHealthScope::Workouts` its workouts as activities.
    pub(super) fn import_apple_health(
        &self,
        archive: &mut ZipArchive<File>,
        scope: AppleHealthScope,
    ) -> Result<ImportReport> {
        let names: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .map(|name| name.into_owned())
            .collect();
        let file_name = |name: &str| name.rsplit('/').next().unwrap_or(name).to_owned();
        let export = names
            .iter()
            .find(|name| file_name(name).eq_ignore_ascii_case("export.xml"))
            .context("there is no Export.xml in this ZIP. Is it an Apple Health export?")?;
        let parsed = {
            let entry = archive.by_name(export)?;
            // The workouts are read for health data too, which links its VO2 max to them.
            apple_health::parse(BufReader::new(entry.take(MAX_ENTRY_BYTES)), true)?
        };

        let mut report = ImportReport::default();
        let routes: HashMap<String, String> = names
            .iter()
            .filter(|name| name.to_ascii_lowercase().ends_with(".gpx"))
            .map(|name| (file_name(name), name.clone()))
            .collect();
        // The activities that the workouts are, by the time they span.
        let mut activities: Vec<(
            DateTime<chrono::FixedOffset>,
            DateTime<chrono::FixedOffset>,
            Uuid,
        )> = Vec::new();
        if scope == AppleHealthScope::Workouts {
            for workout in &parsed.workouts {
                match self.import_workout(archive, workout, &routes) {
                    Ok(Some(id)) => {
                        report.activities += 1;
                        activities.push((workout.start, workout.end, id));
                    }
                    Ok(None) => report.skipped += 1,
                    Err(error) => {
                        tracing::warn!(start = %workout.start, "could not import: {error:#}");
                        report.failed += 1;
                    }
                }
            }
        } else {
            // Workouts from an earlier import still get their VO2 max.
            let garmin_ids: Vec<i64> = parsed
                .workouts
                .iter()
                .map(|workout| self.workout_id(workout))
                .collect();
            let known = self.handle.block_on(database::ids_by_garmin_id(
                self.pool,
                self.user_id,
                &garmin_ids,
            ))?;
            for (workout, garmin_id) in parsed.workouts.iter().zip(garmin_ids) {
                if let Some(id) = known.get(&garmin_id) {
                    activities.push((workout.start, workout.end, *id));
                }
            }
        }

        // Each VO2 max goes to the workout that it ended, if there is one.
        let mut vo2_max: BTreeMap<NaiveDate, Vec<database::Vo2Reading>> = BTreeMap::new();
        for measured in &parsed.vo2_max {
            let slack = chrono::Duration::minutes(VO2_MAX_AFTER_WORKOUT_MINUTES);
            let activity_id = activities
                .iter()
                .filter(|(start, end, _)| *start <= measured.time && measured.time <= *end + slack)
                .max_by_key(|(start, _, _)| *start)
                .map(|(_, _, id)| *id);
            vo2_max
                .entry(measured.time.naive_local().date())
                .or_default()
                .push(database::Vo2Reading {
                    value: measured.value,
                    activity_id,
                });
        }

        let days: BTreeSet<NaiveDate> = parsed.days.keys().chain(vo2_max.keys()).copied().collect();
        let no_metrics = BTreeMap::new();
        for day in days {
            let metrics = parsed.days.get(&day).unwrap_or(&no_metrics);
            let readings = vo2_max.get(&day).map_or(&[][..], Vec::as_slice);
            self.save_apple_day(day, metrics, readings)?;
            report.health_documents += 1;
        }
        Ok(report)
    }

    pub(super) fn save_apple_day(
        &self,
        day: NaiveDate,
        metrics: &BTreeMap<String, f64>,
        vo2_max: &[database::Vo2Reading],
    ) -> Result<()> {
        let mut kept = self.kept.borrow_mut();
        kept.documents
            .insert((apple_health::DOCUMENT_KIND.to_owned(), day.to_string()));
        kept.metrics
            .extend(metrics.keys().map(|metric| (metric.clone(), day)));
        if !vo2_max.is_empty() {
            kept.metrics
                .insert((database::VO2_MAX_METRIC.to_owned(), day));
        }
        drop(kept);
        self.handle.block_on(database::save_apple_health_day(
            self.pool,
            self.user_id,
            day,
            metrics,
            vo2_max,
        ))
    }

    /// The ID that a workout gets, which is the same on every import.
    fn workout_id(&self, workout: &apple_health::Workout) -> i64 {
        let mut identity = self.user_id.to_le_bytes().to_vec();
        identity.extend(b"apple-health");
        identity.extend(workout.start.to_rfc3339().as_bytes());
        identity.extend(workout.activity_type.as_bytes());
        imported_id(&identity)
    }

    /// Imports one workout, with its route if the export has it. Gives the ID of the
    /// activity, or `None` if another account has it.
    fn import_workout(
        &self,
        archive: &mut ZipArchive<File>,
        workout: &apple_health::Workout,
        routes: &HashMap<String, String>,
    ) -> Result<Option<Uuid>> {
        let (type_key, name) = apple_health::activity_type(&workout.activity_type);
        let id = self.workout_id(workout);

        let route = workout
            .route
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .and_then(|file| routes.get(file));
        let original = match route {
            Some(entry) => {
                let gpx = read_entry(archive, entry)?;
                Some(single_file_zip("route.gpx", &gpx)?)
            }
            None => None,
        };
        let samples = original
            .as_deref()
            .and_then(|zip| track::parse_samples(zip).ok())
            .unwrap_or_default();
        let summary = workout_summary(id, &name, &type_key, workout, &samples);
        let activity = Activity::from_summary(summary)?;
        let Some(saved) = self.claim(&activity, None)? else {
            return Ok(None);
        };
        let original = original.filter(|_| !samples.is_empty());
        self.save_track(&activity, saved, original.as_deref())?;
        // Apple has nothing more for a workout, so syncs never ask.
        self.handle
            .block_on(database::save_details(self.pool, saved, &json!({})))?;
        self.handle
            .block_on(database::save_workouts(self.pool, saved, &json!([])))?;
        self.handle
            .block_on(database::save_images(self.pool, saved, &[]))?;
        Ok(Some(saved))
    }
}

/// A summary like Garmin's for a workout of Apple Health. What the workout says
/// wins over what its route's samples add up to.
fn workout_summary(
    id: i64,
    name: &str,
    sport: &str,
    workout: &apple_health::Workout,
    samples: &[Sample],
) -> Value {
    let mut summary = summary_of(id, name, sport, samples).unwrap_or_else(|_| {
        json!({
            "activityId": id,
            "activityName": name,
            "activityType": {"typeKey": sport},
        })
    });
    let fields = summary.as_object_mut().expect("an object");
    let format = "%Y-%m-%d %H:%M:%S";
    fields.insert(
        "startTimeGMT".into(),
        json!(workout.start.with_timezone(&Utc).format(format).to_string()),
    );
    fields.insert(
        "startTimeLocal".into(),
        json!(workout.start.naive_local().format(format).to_string()),
    );
    for key in ["duration", "movingDuration", "elapsedDuration"] {
        fields.insert(key.into(), json!(workout.duration_s));
    }
    if let Some(distance) = workout.distance_m.filter(|distance| *distance > 0.0) {
        fields.insert("distance".into(), json!(distance));
        if workout.duration_s > 0.0 {
            fields.insert("averageSpeed".into(), json!(distance / workout.duration_s));
        }
    } else if !fields.contains_key("distance") {
        fields.insert("distance".into(), json!(0.0));
    }
    if let Some(energy) = workout.energy_kcal {
        fields.insert("calories".into(), json!(energy.round()));
    }
    if let Some(average) = workout.average_heart_rate {
        fields.insert("averageHR".into(), json!(average.round()));
    }
    if let Some(max) = workout.max_heart_rate {
        fields.insert("maxHR".into(), json!(max.round()));
    }
    fields.insert("source".into(), json!("apple_health"));
    summary
}
