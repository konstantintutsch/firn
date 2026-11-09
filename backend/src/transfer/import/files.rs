use std::fs::File;

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use fitparser::profile::MesgNum;
use serde_json::{Value, json};
use zip::ZipArchive;

use super::{Import, ImportReport};
use crate::db::{self};
use crate::garmin::Activity;
use crate::track::{self, Point, Sample};
use crate::transfer::entries::{imported_id, read_limited, single_file_zip};

impl Import<'_> {
    pub(super) fn import_files(&self, archive: &mut ZipArchive<File>) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        for index in 0..archive.len() {
            let (name, content) = {
                let mut entry = archive.by_index(index)?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name()?.into_owned();
                let lower = name.to_ascii_lowercase();
                let wanted = [".fit", ".gpx", ".tcx", ".zip"]
                    .iter()
                    .any(|extension| lower.ends_with(extension));
                if !wanted {
                    continue;
                }
                (name, read_limited(&mut entry)?)
            };
            // Garmin wraps each activity in a ZIP, which is kept as the original.
            let file_name = name.rsplit('/').next().unwrap_or(&name);
            let original = if file_name.to_ascii_lowercase().ends_with(".zip") {
                content
            } else {
                single_file_zip(file_name, &content)?
            };
            let stem = file_name.rsplit_once('.').map_or("", |(stem, _)| stem);
            match self.import_file(stem, &original) {
                Ok(true) => report.activities += 1,
                Ok(false) => report.skipped += 1,
                Err(error) => {
                    tracing::warn!(name, "could not import: {error:#}");
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Imports one activity file, in its ZIP. False if another account has it.
    fn import_file(&self, name: &str, original: &[u8]) -> Result<bool> {
        let samples = track::parse_samples(original)?;
        let (file_name, content) = track::original_file(original)?;
        let sport = sport(&file_name, &content);
        let mut identity = self.user_id.to_le_bytes().to_vec();
        identity.extend(&content);
        let summary = summary_of(imported_id(&identity), name, &sport, &samples)?;
        let activity = Activity::from_summary(summary)?;
        let Some(id) = self.claim(&activity, None)? else {
            return Ok(false);
        };
        self.save_track(&activity, id, Some(original))?;
        // Garmin has nothing more for files from elsewhere, so syncs never ask.
        self.handle
            .block_on(db::save_details(self.pool, id, &json!({})))?;
        self.handle
            .block_on(db::save_workouts(self.pool, id, &json!([])))?;
        self.handle.block_on(db::save_images(self.pool, id, &[]))?;
        Ok(true)
    }
}

/// The sport that a file names, as one of Garmin's type keys where one fits.
fn sport(file_name: &str, content: &[u8]) -> String {
    let named = if file_name.ends_with(".fit") {
        fit_sport(content)
    } else {
        xml_sport(content)
    };
    let named = named.unwrap_or_default().to_ascii_lowercase();
    match named.as_str() {
        "biking" | "cycling" | "bike" | "ride" => "cycling",
        "running" | "run" => "running",
        "walking" | "walk" => "walking",
        "hiking" | "hike" => "hiking",
        "swimming" | "swim" => "open_water_swimming",
        "" | "other" | "generic" => "other",
        other => return other.replace(' ', "_"),
    }
    .to_owned()
}

fn fit_sport(content: &[u8]) -> Option<String> {
    fitparser::from_bytes(content)
        .ok()?
        .into_iter()
        .filter(|record| matches!(record.kind(), MesgNum::Sport | MesgNum::Session))
        .flat_map(|record| record.into_vec())
        .find(|field| field.name() == "sport")
        .map(|field| field.value().to_string())
}

/// GPX names it in `<type>`, and TCX in the `Sport` of an `Activity`.
fn xml_sport(content: &[u8]) -> Option<String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(content);
    let mut buffer = Vec::new();
    let mut in_type = false;
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) if element.local_name().as_ref() == b"type" => in_type = true,
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"Activity" =>
            {
                let sport = element
                    .attributes()
                    .flatten()
                    .find(|attribute| attribute.key.as_ref() == b"Sport")?;
                return Some(String::from_utf8_lossy(&sport.value).into_owned());
            }
            Event::Text(text) if in_type => {
                return Some(String::from_utf8_lossy(&text).trim().to_owned());
            }
            Event::End(_) => in_type = false,
            Event::Eof => return None,
            _ => {}
        }
        buffer.clear();
    }
}

/// A summary like Garmin's, made from the samples of a file from elsewhere.
pub(super) fn summary_of(id: i64, name: &str, sport: &str, samples: &[Sample]) -> Result<Value> {
    let times: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.timestamp)
        .collect();
    let (Some(&start), Some(&end)) = (times.first(), times.last()) else {
        bail!("the file has no times, so its date is unknown");
    };
    let started = DateTime::from_timestamp_millis((start * 1000.0) as i64)
        .context("the file has an invalid time")?;
    let duration = (end - start).max(0.0);
    let positions: Vec<Point> = samples
        .iter()
        .filter_map(|sample| sample.position)
        .collect();
    let distance = samples
        .iter()
        .rev()
        .find_map(|sample| sample.distance_m)
        .unwrap_or_else(|| {
            positions
                .windows(2)
                .map(|pair| meters(pair[0], pair[1]))
                .sum()
        });
    let elevations: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.elevation_m)
        .collect();
    let gain: f64 = elevations
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).max(0.0))
        .sum();
    let rates: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.heart_rate)
        .collect();
    // Garmin's format, in UTC: a file from elsewhere does not say its time zone.
    let time = started.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut summary = json!({
        "activityId": id,
        "activityName": name,
        "activityType": {"typeKey": sport},
        "startTimeGMT": time,
        "startTimeLocal": time,
        "duration": duration,
        "movingDuration": duration,
        "elapsedDuration": duration,
        "distance": distance,
        "source": "import",
    });
    let fields = summary.as_object_mut().expect("an object");
    if let Some(first) = positions.first() {
        fields.insert("startLatitude".into(), json!(first.lat));
        fields.insert("startLongitude".into(), json!(first.lon));
    }
    if duration > 0.0 && distance > 0.0 {
        fields.insert("averageSpeed".into(), json!(distance / duration));
    }
    if !elevations.is_empty() {
        fields.insert("elevationGain".into(), json!(gain.round()));
    }
    if !rates.is_empty() {
        let average = rates.iter().sum::<f64>() / rates.len() as f64;
        fields.insert("averageHR".into(), json!(average.round()));
        fields.insert(
            "maxHR".into(),
            json!(rates.iter().copied().fold(0.0, f64::max)),
        );
    }
    Ok(summary)
}

/// The distance between two points on the earth, in meters.
fn meters(a: Point, b: Point) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sport_of_gpx_and_tcx() {
        let gpx = b"<gpx><trk><name>x</name><type>Biking</type></trk></gpx>";
        assert_eq!(sport("a.gpx", gpx), "cycling");
        let tcx = br#"<TrainingCenterDatabase><Activities><Activity Sport="Running"/></Activities></TrainingCenterDatabase>"#;
        assert_eq!(sport("a.tcx", tcx), "running");
        assert_eq!(sport("a.gpx", b"<gpx/>"), "other");
    }

    #[test]
    fn distance_between_points() {
        let berlin = Point {
            lat: 52.52,
            lon: 13.405,
        };
        let potsdam = Point {
            lat: 52.39,
            lon: 13.065,
        };
        assert!((meters(berlin, potsdam) - 26_900.0).abs() < 500.0);
    }
}
