//! Exports an account's data to a ZIP, and imports such a ZIP or one with loose
//! FIT, GPX and TCX files from other apps and devices.
//!
//! An export holds:
//! - `runmap-export.json`, which marks it, with the format version,
//! - `activities/<id>/activity.json` with the activity's ID, the summary,
//!   details, workouts and the list of images,
//! - `activities/<id>/original.zip`, the file as Garmin sent it,
//! - `activities/<id>/images/<n>`, the photos in the order of the list,
//! - `routes/<id>.json`, one saved route with its points,
//! - `health.jsonl`, one Garmin health document per line.
//!
//! Both run on a blocking thread, since ZIP files are read and written
//! synchronously. They reach the database through the runtime's handle.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate};
use fitparser::profile::MesgNum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::runtime::Handle;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::db::{self, Pool};
use crate::files::FileStore;
use crate::garmin::Activity;
use crate::routes;
use crate::secret;
use crate::track::{self, Point, Sample};
use crate::wellness::KINDS;

const MARKER: &str = "runmap-export.json";
const FORMAT_VERSION: u32 = 1;
/// Activities read from the database at a time, to keep memory use low.
const ACTIVITIES_PER_QUERY: usize = 100;
const DOCUMENTS_PER_QUERY: i64 = 500;
/// Far above any real activity file, but protects against a corrupt archive.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Marker {
    format: String,
    version: u32,
    exported_at: DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize)]
struct ExportedActivity {
    /// Missing in exports from before activities had IDs of their own.
    #[serde(default)]
    id: Option<Uuid>,
    summary: Value,
    details: Option<Value>,
    workouts: Option<Value>,
    #[serde(default)]
    images: Vec<ExportedImage>,
    #[serde(default)]
    result: Option<db::RaceResult>,
}

#[derive(Serialize, Deserialize)]
struct ExportedImage {
    id: String,
    content_type: String,
}

#[derive(Serialize, Deserialize)]
struct ExportedDocument {
    kind: String,
    period: String,
    data: Option<Value>,
}

/// Writes all of the account's data as a ZIP to `out`.
pub fn export(
    handle: &Handle,
    pool: &Pool,
    files: &FileStore,
    user_id: i64,
    out: impl Write + Seek,
) -> Result<()> {
    let mut zip = ZipWriter::new(out);
    // Original files are ZIPs and photos are compressed already.
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default();

    zip.start_file(MARKER, deflated)?;
    serde_json::to_writer_pretty(
        &mut zip,
        &Marker {
            format: "runmap-export".to_owned(),
            version: FORMAT_VERSION,
            exported_at: chrono::Utc::now(),
        },
    )?;

    let ids = handle.block_on(db::activity_ids(pool, user_id))?;
    for chunk in ids.chunks(ACTIVITIES_PER_QUERY) {
        let mut results = handle.block_on(db::race_results(pool, chunk))?;
        for activity in handle.block_on(db::activity_exports(pool, user_id, chunk))? {
            let dir = format!("activities/{}", activity.id);
            zip.start_file(format!("{dir}/activity.json"), deflated)?;
            serde_json::to_writer(
                &mut zip,
                &ExportedActivity {
                    id: Some(activity.id),
                    summary: activity.summary,
                    details: activity.details,
                    workouts: activity.workouts,
                    images: activity
                        .images
                        .iter()
                        .map(|(id, content_type)| ExportedImage {
                            id: id.clone(),
                            content_type: content_type.clone(),
                        })
                        .collect(),
                    result: results.remove(&activity.id),
                },
            )?;
            if let Some(original) = handle.block_on(files.original(activity.id))? {
                zip.start_file(format!("{dir}/original.zip"), stored)?;
                zip.write_all(&original)?;
            }
            for (index, (image_id, _)) in activity.images.iter().enumerate() {
                if let Some(image) = handle.block_on(files.image(activity.id, image_id))? {
                    zip.start_file(format!("{dir}/images/{index}"), stored)?;
                    zip.write_all(&image)?;
                }
            }
        }
    }

    for route in handle.block_on(db::route_records(pool, user_id))? {
        zip.start_file(format!("routes/{}.json", route.id), deflated)?;
        serde_json::to_writer(&mut zip, &route)?;
    }

    zip.start_file("health.jsonl", deflated)?;
    let mut after = None;
    loop {
        let page = handle.block_on(db::documents(pool, user_id, after, DOCUMENTS_PER_QUERY))?;
        let Some((kind, period, _)) = page.last() else {
            break;
        };
        after = Some((kind.clone(), period.clone()));
        for (kind, period, data) in page {
            serde_json::to_writer(&mut zip, &ExportedDocument { kind, period, data })?;
            zip.write_all(b"\n")?;
        }
    }
    zip.finish()?;
    Ok(())
}

#[derive(Debug, Default, Serialize, PartialEq, utoipa::ToSchema)]
pub struct ImportReport {
    /// Activities that were saved or updated.
    pub activities: usize,
    /// Activities and routes that another account saved already, which stay with it.
    pub skipped: usize,
    /// Files that could not be read, like broken ones or ones without a time.
    pub failed: usize,
    /// Health documents, like a day of sleep.
    pub health_documents: usize,
    /// Routes that were saved or updated.
    pub routes: usize,
    /// Activities that were removed because they were not in the import, when
    /// the import replaces what the account had.
    pub removed_activities: usize,
    /// Routes that were removed because they were not in the import.
    pub removed_routes: usize,
    /// Health documents that were removed because they were not in the import.
    pub removed_health_documents: usize,
    /// True if the import should have replaced the account's data but did not,
    /// since some files could not be read and would have been lost.
    pub replace_skipped: bool,
}

/// What an import saved, which a replacing import keeps.
#[derive(Default)]
struct Kept {
    activities: HashSet<Uuid>,
    routes: HashSet<Uuid>,
    documents: HashSet<(String, String)>,
    metrics: HashSet<(String, NaiveDate)>,
}

/// Imports a Runmap export, or a ZIP with FIT, GPX or TCX files, which may also
/// be inside ZIPs of their own, like in Garmin's download of an activity.
///
/// With `replace`, the account ends up with what the import holds and nothing
/// else: activities, routes and health data that were not in it are removed,
/// unless some of its files could not be read.
pub fn import(
    handle: &Handle,
    pool: &Pool,
    files: &FileStore,
    user_id: i64,
    path: &Path,
    replace: bool,
) -> Result<ImportReport> {
    let mut archive = ZipArchive::new(File::open(path)?).context("this is not a ZIP file")?;
    let context = Import {
        handle,
        pool,
        files,
        user_id,
        kept: RefCell::default(),
    };
    let mut report = if archive.index_for_name(MARKER).is_some() {
        context.import_export(&mut archive)?
    } else {
        context.import_files(&mut archive)?
    };
    if replace {
        if report.failed > 0 {
            report.replace_skipped = true;
        } else {
            context.remove_the_rest(&mut report)?;
        }
    }
    Ok(report)
}

/// What an import needs everywhere.
struct Import<'a> {
    handle: &'a Handle,
    pool: &'a Pool,
    files: &'a FileStore,
    user_id: i64,
    kept: RefCell<Kept>,
}

impl Import<'_> {
    /// Removes the account's activities, routes and health data that the import did not hold.
    fn remove_the_rest(&self, report: &mut ImportReport) -> Result<()> {
        let kept = self.kept.borrow();
        let activities: Vec<Uuid> = self
            .handle
            .block_on(db::activity_ids(self.pool, self.user_id))?
            .into_iter()
            .filter(|id| !kept.activities.contains(id))
            .collect();
        for chunk in activities.chunks(ACTIVITIES_PER_QUERY) {
            // The files first: once the rows are gone, nothing says which files were these.
            for &id in chunk {
                self.handle.block_on(self.files.remove_activity(id))?;
            }
            self.handle
                .block_on(db::delete_activities(self.pool, self.user_id, chunk))?;
        }
        report.removed_activities = activities.len();

        let routes: Vec<Uuid> = self
            .handle
            .block_on(db::route_ids(self.pool, self.user_id))?
            .into_iter()
            .filter(|id| !kept.routes.contains(id))
            .collect();
        self.handle
            .block_on(db::delete_routes(self.pool, self.user_id, &routes))?;
        report.removed_routes = routes.len();

        let mut documents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (kind, period) in self
            .handle
            .block_on(db::document_keys(self.pool, self.user_id))?
        {
            if !kept.documents.contains(&(kind.clone(), period.clone())) {
                documents.entry(kind).or_default().push(period);
            }
        }
        for (kind, periods) in &documents {
            for chunk in periods.chunks(DOCUMENTS_PER_QUERY as usize) {
                self.handle
                    .block_on(db::delete_documents(self.pool, self.user_id, kind, chunk))?;
            }
            report.removed_health_documents += periods.len();
        }

        let mut metrics: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
        for (metric, day) in self
            .handle
            .block_on(db::metric_keys(self.pool, self.user_id))?
        {
            if !kept.metrics.contains(&(metric.clone(), day)) {
                metrics.entry(metric).or_default().push(day);
            }
        }
        for (metric, days) in &metrics {
            for chunk in days.chunks(DOCUMENTS_PER_QUERY as usize) {
                self.handle
                    .block_on(db::delete_metrics(self.pool, self.user_id, metric, chunk))?;
            }
        }
        Ok(())
    }

    fn import_export(&self, archive: &mut ZipArchive<File>) -> Result<ImportReport> {
        let marker: Marker = serde_json::from_slice(&read_entry(archive, MARKER)?)
            .context("unreadable runmap-export.json")?;
        if marker.format != "runmap-export" || marker.version > FORMAT_VERSION {
            bail!("this export is from a newer version of Runmap");
        }
        let activity_files: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .filter(|name| name.starts_with("activities/") && name.ends_with("/activity.json"))
            .map(|name| name.into_owned())
            .collect();

        let mut report = ImportReport::default();
        for name in activity_files {
            let dir = name.trim_end_matches("/activity.json");
            match self.import_exported_activity(archive, dir) {
                Ok(true) => report.activities += 1,
                Ok(false) => report.skipped += 1,
                Err(error) => {
                    tracing::warn!(dir, "could not import: {error:#}");
                    report.failed += 1;
                }
            }
        }
        let route_files: Vec<String> = archive
            .file_names()
            .filter_map(Result::ok)
            .filter(|name| name.starts_with("routes/") && name.ends_with(".json"))
            .map(|name| name.into_owned())
            .collect();
        for name in route_files {
            match self.import_exported_route(archive, &name) {
                Ok(true) => report.routes += 1,
                Ok(false) => report.skipped += 1,
                Err(error) => {
                    tracing::warn!(name, "could not import: {error:#}");
                    report.failed += 1;
                }
            }
        }
        if archive.index_for_name("health.jsonl").is_some() {
            let lines = read_entry(archive, "health.jsonl")?;
            for line in lines.split(|byte| *byte == b'\n') {
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let document: ExportedDocument =
                    serde_json::from_slice(line).context("unreadable health.jsonl")?;
                self.save_document(&document)?;
                report.health_documents += 1;
            }
        }
        Ok(report)
    }

    /// False if another account has this route.
    fn import_exported_route(&self, archive: &mut ZipArchive<File>, name: &str) -> Result<bool> {
        let mut route: db::RouteRecord = serde_json::from_slice(&read_entry(archive, name)?)?;
        let points: Vec<(f64, f64, Option<f64>)> = serde_json::from_value(route.points)?;
        let prepared = routes::prepare(points).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        route.points = serde_json::to_value(&prepared.points)?;
        route.distance_m = prepared.distance_m;
        route.elevation_gain_m = prepared.elevation_gain_m;
        let preview = serde_json::to_value(&prepared.preview)?;
        let id = route.id;
        let saved =
            self.handle
                .block_on(db::import_route(self.pool, self.user_id, &route, &preview))?;
        if saved {
            self.kept.borrow_mut().routes.insert(id);
        }
        Ok(saved)
    }

    /// False if another account has this activity.
    fn import_exported_activity(&self, archive: &mut ZipArchive<File>, dir: &str) -> Result<bool> {
        let exported: ExportedActivity =
            serde_json::from_slice(&read_entry(archive, &format!("{dir}/activity.json"))?)?;
        let activity = Activity::from_summary(exported.summary)?;
        let Some(id) = self.claim(&activity, exported.id)? else {
            return Ok(false);
        };
        let original = optional_entry(archive, &format!("{dir}/original.zip"))?;
        self.save_track(&activity, id, original.as_deref())?;
        if let Some(details) = &exported.details {
            self.handle
                .block_on(db::save_details(self.pool, id, details))?;
        }
        if let Some(workouts) = &exported.workouts {
            self.handle
                .block_on(db::save_workouts(self.pool, id, workouts))?;
        }
        let mut images = Vec::new();
        let mut saved = Vec::new();
        for (index, image) in exported.images.into_iter().enumerate() {
            if let Some(data) = optional_entry(archive, &format!("{dir}/images/{index}"))? {
                images.push((image.id.clone(), data));
                saved.push(db::Image {
                    id: image.id,
                    content_type: image.content_type,
                });
            }
        }
        let images: Vec<(&str, &[u8])> = images
            .iter()
            .map(|(id, data)| (id.as_str(), data.as_slice()))
            .collect();
        self.handle.block_on(self.files.save_images(id, &images))?;
        self.handle
            .block_on(db::save_images(self.pool, id, &saved))?;
        if let Some(result) = &exported.result {
            self.handle
                .block_on(db::save_race_result(self.pool, self.user_id, id, result))?;
        }
        Ok(true)
    }

    fn import_files(&self, archive: &mut ZipArchive<File>) -> Result<ImportReport> {
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

    /// Saves the activity for the account and gives its ID, unless another
    /// account has it. A new activity takes `id` if there is one.
    fn claim(&self, activity: &Activity, id: Option<Uuid>) -> Result<Option<Uuid>> {
        if let Some(Some(owner)) = self.handle.block_on(db::owner(self.pool, activity.id))?
            && owner != self.user_id
        {
            return Ok(None);
        }
        self.handle.block_on(db::upsert_activities_with_ids(
            self.pool,
            self.user_id,
            &[(activity, id)],
        ))?;
        let mut ids = self.handle.block_on(db::ids_by_garmin_id(
            self.pool,
            self.user_id,
            &[activity.id],
        ))?;
        let id = ids.remove(&activity.id);
        if let Some(id) = id {
            self.kept.borrow_mut().activities.insert(id);
        }
        Ok(id)
    }

    fn save_track(&self, activity: &Activity, id: Uuid, original: Option<&[u8]>) -> Result<()> {
        let points = match original {
            Some(zip) if activity.has_track => track::parse_archive(zip)?,
            _ => Vec::new(),
        };
        self.handle
            .block_on(self.files.save_original(id, original))?;
        self.handle
            .block_on(db::save_track(self.pool, id, &points))?;
        Ok(())
    }

    fn save_document(&self, document: &ExportedDocument) -> Result<()> {
        let metrics = match (
            KINDS.iter().find(|kind| kind.name == document.kind),
            &document.data,
        ) {
            (Some(kind), Some(data)) => {
                let day = NaiveDate::parse_from_str(&document.period, "%Y-%m-%d").ok();
                kind.extract(data, day)
            }
            _ => Vec::new(),
        };
        let mut kept = self.kept.borrow_mut();
        kept.documents
            .insert((document.kind.clone(), document.period.clone()));
        kept.metrics.extend(
            metrics
                .iter()
                .map(|(day, metric, _)| ((*metric).to_owned(), *day)),
        );
        drop(kept);
        self.handle.block_on(db::save_document(
            self.pool,
            self.user_id,
            &document.kind,
            &document.period,
            document.data.as_ref(),
            &metrics,
        ))
    }
}

fn read_limited(entry: &mut impl Read) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    entry
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut content)
        .context("could not extract ZIP entry")?;
    if content.len() as u64 > MAX_ENTRY_BYTES {
        bail!("a file in the archive is too large");
    }
    Ok(content)
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>> {
    read_limited(&mut archive.by_name(name)?)
}

fn optional_entry(archive: &mut ZipArchive<File>, name: &str) -> Result<Option<Vec<u8>>> {
    if archive.index_for_name(name).is_none() {
        return Ok(None);
    }
    read_entry(archive, name).map(Some)
}

fn single_file_zip(name: &str, content: &[u8]) -> Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(name, SimpleFileOptions::default())?;
    zip.write_all(content)?;
    Ok(zip.finish()?.into_inner())
}

/// An ID for a file from elsewhere, from a hash of it. Negative, so that it never
/// meets one of Garmin's, and the same for the same file, so that importing it
/// again updates it. Within 53 bits, which browsers read as numbers exactly.
fn imported_id(identity: &[u8]) -> i64 {
    const MAX_EXACT_IN_JAVASCRIPT: i64 = (1 << 53) - 1;
    let hash = secret::sha256_bytes(identity);
    let number =
        i64::from_le_bytes(hash[..8].try_into().expect("8 bytes")) & MAX_EXACT_IN_JAVASCRIPT;
    -number.max(1)
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
fn summary_of(id: i64, name: &str, sport: &str, samples: &[Sample]) -> Result<Value> {
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
    fn imported_ids_are_negative_and_stable() {
        let id = imported_id(b"file");
        assert!(id < 0 && id > -(1 << 53));
        assert_eq!(id, imported_id(b"file"));
        assert_ne!(id, imported_id(b"other file"));
    }

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
