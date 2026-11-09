use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::db::{self};

pub(super) const MARKER: &str = "firn-export.json";
/// Exports made before the project was renamed carry this marker and format.
pub(super) const LEGACY_MARKER: &str = "runmap-export.json";
pub(super) const LEGACY_FORMAT: &str = "runmap-export";
pub(super) const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub(super) struct Marker {
    pub(super) format: String,
    pub(super) version: u32,
    pub(super) exported_at: DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ExportedActivity {
    /// Missing in exports from before activities had IDs of their own.
    #[serde(default)]
    pub(super) id: Option<Uuid>,
    pub(super) summary: Value,
    pub(super) details: Option<Value>,
    pub(super) workouts: Option<Value>,
    #[serde(default)]
    pub(super) images: Vec<ExportedImage>,
    #[serde(default)]
    pub(super) result: Option<db::RaceResult>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ExportedImage {
    pub(super) id: String,
    pub(super) content_type: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ExportedDocument {
    pub(super) kind: String,
    pub(super) period: String,
    pub(super) data: Option<Value>,
}
