//! Client for the Garmin Connect Personal API.
//!
//! See <https://developer.garmin.com/gc-developer-program/personal/swagger-docs/>.
mod activity;
mod client;
mod error;

pub use activity::*;
pub use client::{DEFAULT_BASE_URL, GarminClient, image_type};
pub use error::GarminError;
