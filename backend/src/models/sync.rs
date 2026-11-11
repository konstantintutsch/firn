use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::sync::Report;

#[derive(Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// A running sync lives in memory. The last finished one is also saved with the
/// account, so that it survives a restart. After a restart, the next sync finds
/// what is still missing.
#[derive(Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct SyncStatus {
    pub state: SyncState,
    /// When the sync ended, for finished syncs.
    pub finished_at: Option<DateTime<Utc>>,
    /// Activities found on Garmin so far.
    pub listed: usize,
    /// Activities that this sync has to download.
    pub to_download: usize,
    pub downloaded: usize,
    /// Activities that could not be saved. The next sync retries them.
    pub failed: usize,
    /// Requests for health data, like sleep or HRV, that this sync has to make.
    pub health_to_fetch: usize,
    pub health_fetched: usize,
    /// Health data that could not be saved. The next sync retries it.
    pub health_failed: usize,
    pub error: Option<String>,
}

impl SyncStatus {
    pub fn running(report: &Report) -> Self {
        Self {
            state: SyncState::Running,
            listed: report.listed,
            to_download: report.to_download,
            downloaded: report.downloaded,
            failed: report.failed,
            health_to_fetch: report.health_to_fetch,
            health_fetched: report.health_fetched,
            health_failed: report.health_failed,
            error: None,
            finished_at: None,
        }
    }
}
