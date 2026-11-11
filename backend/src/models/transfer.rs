use serde::{Deserialize, Serialize};
use utoipa::IntoParams;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportQuery {
    /// Leaves the account with what the import holds and nothing else: its
    /// other activities, routes and health data are removed. Does nothing if
    /// some files of the import could not be read. Defaults to `false`.
    pub replace: Option<bool>,
    /// Reads the ZIP as the export of the Apple Health app: `health` for only
    /// its daily values, like steps, sleep and heart rate, or `workouts` for
    /// those and its workouts, which become activities. Values that Garmin
    /// saved already stay.
    pub apple_health: Option<AppleHealthScope>,
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

/// What to read from an Apple Health export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AppleHealthScope {
    /// Only the daily health values, like steps, sleep and heart rate.
    Health,
    /// The health values and the workouts, which become activities.
    Workouts,
}
