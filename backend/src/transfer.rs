//! Exports an account's data to a ZIP, and imports such a ZIP or one with loose
//! FIT, GPX and TCX files from other apps and devices.
//!
//! An export holds:
//! - `firn-export.json`, which marks it, with the format version,
//! - `activities/<id>/activity.json` with the activity's ID, the summary,
//!   details, workouts and the list of images,
//! - `activities/<id>/original.zip`, the file as Garmin sent it,
//! - `activities/<id>/images/<n>`, the photos in the order of the list,
//! - `routes/<id>.json`, one saved route with its points,
//! - `health.jsonl`, one Garmin health document per line.
//!
//! Both run on a blocking thread, since ZIP files are read and written
//! synchronously. They reach the database through the runtime's handle.

mod entries;
mod export;
mod format;
mod import;

pub use export::export;
pub use import::import;

/// Activities read from the database at a time, to keep memory use low.
const ACTIVITIES_PER_QUERY: usize = 100;
const DOCUMENTS_PER_QUERY: i64 = 500;
