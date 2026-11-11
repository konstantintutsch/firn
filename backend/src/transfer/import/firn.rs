use std::fs::File;

use anyhow::{Context, Result, bail};
use zip::ZipArchive;

use super::{Import, ImportReport};
use crate::database;
use crate::garmin::{Activity, image_type};
use crate::models;
use crate::routes;
use crate::transfer::entries::{optional_entry, read_entry};
use crate::transfer::format::{
    ExportedActivity, ExportedDocument, FORMAT_VERSION, LEGACY_FORMAT, Marker,
};

impl Import<'_> {
    pub(super) fn import_export(
        &self,
        archive: &mut ZipArchive<File>,
        name: &str,
    ) -> Result<ImportReport> {
        let marker: Marker = serde_json::from_slice(&read_entry(archive, name)?)
            .context("unreadable export marker")?;
        if !(marker.format == "firn-export" || marker.format == LEGACY_FORMAT)
            || marker.version > FORMAT_VERSION
        {
            bail!("this export is from a newer version of Firn");
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
    pub(super) fn import_exported_route(
        &self,
        archive: &mut ZipArchive<File>,
        name: &str,
    ) -> Result<bool> {
        let mut route: models::RouteRecord = serde_json::from_slice(&read_entry(archive, name)?)?;
        let points: Vec<(f64, f64, Option<f64>)> = serde_json::from_value(route.points)?;
        let prepared = routes::prepare(points).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        route.points = serde_json::to_value(&prepared.points)?;
        route.distance_m = prepared.distance_m;
        route.elevation_gain_m = prepared.elevation_gain_m;
        let preview = serde_json::to_value(&prepared.preview)?;
        let id = route.id;
        let saved = self.handle.block_on(database::import_route(
            self.pool,
            self.user_id,
            &route,
            &preview,
        ))?;
        if saved {
            self.kept.borrow_mut().routes.insert(id);
        }
        Ok(saved)
    }

    /// False if another account has this activity.
    pub(super) fn import_exported_activity(
        &self,
        archive: &mut ZipArchive<File>,
        dir: &str,
    ) -> Result<bool> {
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
                .block_on(database::save_details(self.pool, id, details))?;
        }
        if let Some(workouts) = &exported.workouts {
            self.handle
                .block_on(database::save_workouts(self.pool, id, workouts))?;
        }
        let mut images = Vec::new();
        let mut saved = Vec::new();
        for (index, image) in exported.images.into_iter().enumerate() {
            if let Some(data) = optional_entry(archive, &format!("{dir}/images/{index}"))? {
                images.push((image.id.clone(), data));
                saved.push(models::Image {
                    id: image.id,
                    // The archive may come from anywhere, so it does not choose freely.
                    content_type: image_type(Some(&image.content_type)).to_owned(),
                });
            }
        }
        let images: Vec<(&str, &[u8])> = images
            .iter()
            .map(|(id, data)| (id.as_str(), data.as_slice()))
            .collect();
        self.handle.block_on(self.files.save_images(id, &images))?;
        self.handle
            .block_on(database::save_images(self.pool, id, &saved))?;
        if let Some(result) = &exported.result {
            self.handle.block_on(database::save_race_result(
                self.pool,
                self.user_id,
                id,
                result,
            ))?;
        }
        Ok(true)
    }
}
