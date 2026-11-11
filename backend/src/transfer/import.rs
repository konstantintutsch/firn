use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use uuid::Uuid;
use zip::ZipArchive;

mod apple;
mod files;
mod firn;

use super::format::{ExportedDocument, LEGACY_MARKER, MARKER};
use super::{ACTIVITIES_PER_QUERY, DOCUMENTS_PER_QUERY};
use crate::apple_health;
use crate::database::{self, Pool};
use crate::files::FileStore;
use crate::garmin::Activity;
use crate::track::{self};
use crate::wellness::KINDS;

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

/// What to read from an Apple Health export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AppleHealthScope {
    /// Only the daily health values, like steps, sleep and heart rate.
    Health,
    /// The health values and the workouts, which become activities.
    Workouts,
}

/// What an import saved, which a replacing import keeps.
#[derive(Default)]
struct Kept {
    activities: HashSet<Uuid>,
    routes: HashSet<Uuid>,
    documents: HashSet<(String, String)>,
    metrics: HashSet<(String, NaiveDate)>,
}

/// Imports a Firn export, or a ZIP with FIT, GPX or TCX files, which may also
/// be inside ZIPs of their own, like in Garmin's download of an activity.
///
/// With `apple_health`, the ZIP is the export of the Apple Health app instead.
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
    apple_health: Option<AppleHealthScope>,
) -> Result<ImportReport> {
    let mut archive = ZipArchive::new(File::open(path)?).context("this is not a ZIP file")?;
    let context = Import {
        handle,
        pool,
        files,
        user_id,
        kept: RefCell::default(),
    };
    let mut report = if let Some(scope) = apple_health {
        context.import_apple_health(&mut archive, scope)?
    } else if let Some(marker) = [MARKER, LEGACY_MARKER]
        .into_iter()
        .find(|name| archive.index_for_name(name).is_some())
    {
        context.import_export(&mut archive, marker)?
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
pub(super) struct Import<'a> {
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
            .block_on(database::activity_ids(self.pool, self.user_id))?
            .into_iter()
            .filter(|id| !kept.activities.contains(id))
            .collect();
        for chunk in activities.chunks(ACTIVITIES_PER_QUERY) {
            // The files first: once the rows are gone, nothing says which files were these.
            for &id in chunk {
                self.handle.block_on(self.files.remove_activity(id))?;
            }
            self.handle
                .block_on(database::delete_activities(self.pool, self.user_id, chunk))?;
        }
        report.removed_activities = activities.len();

        let routes: Vec<Uuid> = self
            .handle
            .block_on(database::route_ids(self.pool, self.user_id))?
            .into_iter()
            .filter(|id| !kept.routes.contains(id))
            .collect();
        self.handle
            .block_on(database::delete_routes(self.pool, self.user_id, &routes))?;
        report.removed_routes = routes.len();

        let mut documents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (kind, period) in self
            .handle
            .block_on(database::document_keys(self.pool, self.user_id))?
        {
            if !kept.documents.contains(&(kind.clone(), period.clone())) {
                documents.entry(kind).or_default().push(period);
            }
        }
        for (kind, periods) in &documents {
            for chunk in periods.chunks(DOCUMENTS_PER_QUERY as usize) {
                self.handle.block_on(database::delete_documents(
                    self.pool,
                    self.user_id,
                    kind,
                    chunk,
                ))?;
            }
            report.removed_health_documents += periods.len();
        }

        let mut metrics: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
        for (metric, day) in self
            .handle
            .block_on(database::metric_keys(self.pool, self.user_id))?
        {
            if !kept.metrics.contains(&(metric.clone(), day)) {
                metrics.entry(metric).or_default().push(day);
            }
        }
        for (metric, days) in &metrics {
            for chunk in days.chunks(DOCUMENTS_PER_QUERY as usize) {
                self.handle.block_on(database::delete_metrics(
                    self.pool,
                    self.user_id,
                    metric,
                    chunk,
                ))?;
            }
        }
        Ok(())
    }

    /// Saves the activity for the account and gives its ID, unless another
    /// account has it. A new activity takes `id` if there is one.
    fn claim(&self, activity: &Activity, id: Option<Uuid>) -> Result<Option<Uuid>> {
        if let Some(Some(owner)) = self
            .handle
            .block_on(database::owner(self.pool, activity.id))?
            && owner != self.user_id
        {
            return Ok(None);
        }
        self.handle.block_on(database::upsert_activities_with_ids(
            self.pool,
            self.user_id,
            &[(activity, id)],
        ))?;
        let mut ids = self.handle.block_on(database::ids_by_garmin_id(
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
            .block_on(database::save_track(self.pool, id, &points))?;
        Ok(())
    }

    fn save_document(&self, document: &ExportedDocument) -> Result<()> {
        if document.kind == apple_health::DOCUMENT_KIND {
            let day = NaiveDate::parse_from_str(&document.period, "%Y-%m-%d")?;
            let metrics: BTreeMap<String, f64> = document
                .data
                .as_ref()
                .and_then(|data| data.get("metrics"))
                .map(|metrics| serde_json::from_value(metrics.clone()))
                .transpose()?
                .unwrap_or_default();
            let vo2_max: Vec<database::Vo2Reading> = document
                .data
                .as_ref()
                .and_then(|data| data.get("vo2_max"))
                .map(|readings| serde_json::from_value(readings.clone()))
                .transpose()?
                .unwrap_or_default();
            return self.save_apple_day(day, &metrics, &vo2_max);
        }
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
        self.handle.block_on(database::save_document(
            self.pool,
            self.user_id,
            &document.kind,
            &document.period,
            document.data.as_ref(),
            &metrics,
        ))
    }
}
