use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::GarminError;

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub id: i64,
    pub name: String,
    pub activity_type: String,
    pub started_at: DateTime<Utc>,
    pub distance_m: Option<f64>,
    pub duration_s: Option<f64>,
    /// False for indoor activities. Virtual ones also count as false: their route
    /// is in a virtual world, not where the athlete was.
    pub has_track: bool,
    pub summary: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActivity {
    activity_id: i64,
    activity_name: Option<String>,
    #[serde(rename = "startTimeGMT")]
    start_time_gmt: String,
    activity_type: RawActivityType,
    distance: Option<f64>,
    duration: Option<f64>,
    has_polyline: Option<bool>,
    start_latitude: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActivityType {
    type_key: String,
}

impl Activity {
    /// Reads an activity from the summary that Garmin's activity list sends.
    pub fn from_summary(summary: Value) -> Result<Self, GarminError> {
        let raw = RawActivity::deserialize(&summary)
            .map_err(|error| GarminError::Unexpected(format!("activity: {error}")))?;
        let started_at = NaiveDateTime::parse_from_str(&raw.start_time_gmt, "%Y-%m-%d %H:%M:%S")
            .map_err(|error| GarminError::Unexpected(format!("activity start: {error}")))?
            .and_utc();
        Ok(Self {
            id: raw.activity_id,
            name: raw.activity_name.unwrap_or_default(),
            // The Personal API leaves hasPolyline out, but only sets a start with GPS.
            has_track: raw.has_polyline.unwrap_or(raw.start_latitude.is_some())
                && !raw.activity_type.type_key.starts_with("virtual"),
            activity_type: raw.activity_type.type_key,
            started_at,
            distance_m: raw.distance,
            duration_s: raw.duration,
            summary,
        })
    }
}

/// A photo that someone added to an activity on Garmin Connect.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityImage {
    pub id: String,
    /// The largest size. Expires a day after Garmin sent it.
    pub url: String,
}

/// The photos in an activity's details, in Garmin's order.
pub fn activity_images(details: &Value) -> Vec<ActivityImage> {
    let Some(images) = details
        .pointer("/metadataDTO/activityImages")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    images
        .iter()
        .filter_map(|image| {
            let url = ["url", "mediumUrl", "smallUrl"]
                .iter()
                .find_map(|key| image.get(key)?.as_str())?;
            Some(ActivityImage {
                id: image.get("imageId")?.as_str()?.to_owned(),
                url: url.to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_largest_size_of_each_image() {
        let details = json!({"metadataDTO": {"activityImages": [
            {"imageId": "a", "url": "https://images/a-larg.jpg", "smallUrl": "https://images/a-small.jpg"},
            {"imageId": "b", "mediumUrl": "https://images/b-mdfd.jpg"},
            {"url": "https://images/no-id.jpg"},
        ]}});
        assert_eq!(
            activity_images(&details),
            [
                ActivityImage {
                    id: "a".into(),
                    url: "https://images/a-larg.jpg".into()
                },
                ActivityImage {
                    id: "b".into(),
                    url: "https://images/b-mdfd.jpg".into()
                },
            ]
        );
        assert!(activity_images(&json!({})).is_empty());
    }

    #[test]
    fn indoor_and_virtual_activities_have_no_track() {
        let activity = |type_key: &str, polyline: Value, latitude: Value| {
            Activity::from_summary(json!({
                "activityId": 1,
                "startTimeGMT": "2026-01-02 03:04:05",
                "activityType": {"typeKey": type_key},
                "hasPolyline": polyline,
                "startLatitude": latitude,
            }))
            .unwrap()
        };
        assert!(activity("running", json!(true), Value::Null).has_track);
        assert!(activity("cycling", Value::Null, json!(52.5)).has_track);
        assert!(!activity("strength_training", Value::Null, Value::Null).has_track);
        assert!(!activity("treadmill_running", json!(false), Value::Null).has_track);
        assert!(!activity("virtual_run", json!(true), json!(52.5)).has_track);
        assert!(!activity("virtual_ride", json!(true), json!(52.5)).has_track);
    }
}
