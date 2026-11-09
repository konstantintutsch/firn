use anyhow::{Context, Result};
use fitparser::Value;
use fitparser::profile::MesgNum;

use super::original_file;

/// A watch or sensor that recorded an activity.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct Device {
    /// Whether this is the device that made the file, usually the watch.
    pub creator: bool,
    /// Like `garmin`.
    pub manufacturer: Option<String>,
    /// The product's name, like `fenix7`, or its number.
    pub product: Option<String>,
    /// What it measures, like `heart_rate` or `bike_power`. Not set for the watch.
    pub kind: Option<String>,
}

/// The types of device that a sensor can have, which the watch's own parts, like
/// its barometer, GPS or sensor hub, do not.
const SENSOR_KINDS: &[&str] = &[
    "heart_rate",
    "bike_power",
    "bike_speed_cadence",
    "bike_speed",
    "bike_cadence",
    "stride_speed_distance",
    "footpod",
    "muscle_oxygen",
    "temperature",
];

/// The text of a FIT field that is not empty.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::Invalid => None,
        other => Some(other.to_string().trim().to_owned()).filter(|text| !text.is_empty()),
    }
}

/// Garmin products that are newer than the FIT profile of the parser, which lists
/// them by number only. Their names come from Garmin's own export of FIT files.
const NEWER_GARMIN_PRODUCTS: &[(&str, &str)] = &[("4607", "hrm_600")];

/// The product's name, or the number again if it is not known.
fn product_name(manufacturer: Option<&str>, product: String) -> String {
    if manufacturer == Some("garmin")
        && let Some((_, name)) = NEWER_GARMIN_PRODUCTS
            .iter()
            .find(|(number, _)| *number == product)
    {
        return (*name).to_owned();
    }
    product
}

/// The watch and the sensors that a FIT file lists, the watch first. A sensor has
/// a product and one of the `SENSOR_KINDS`. Each product of a maker counts once,
/// however many times or with whatever serial number the file lists it.
/// Other formats list none.
pub fn devices(zip: &[u8]) -> Result<Vec<Device>> {
    let (name, content) = original_file(zip)?;
    if !name.ends_with(".fit") {
        return Ok(Vec::new());
    }
    let mut devices: Vec<Device> = Vec::new();
    for record in fitparser::from_bytes(&content).context("malformed FIT")? {
        if record.kind() != MesgNum::DeviceInfo {
            continue;
        }
        let field = |names: &[&str]| {
            names.iter().find_map(|wanted| {
                let field = record
                    .fields()
                    .iter()
                    .find(|field| field.name() == *wanted)?;
                text(field.value())
            })
        };
        let creator =
            field(&["device_index"]).is_some_and(|index| index == "creator" || index == "0");
        let manufacturer = field(&["manufacturer"]);
        let product = field(&[
            "product_name",
            "garmin_product",
            "favero_product",
            "product",
        ])
        .map(|product| product_name(manufacturer.as_deref(), product));
        // The name of the type depends on how the sensor is connected.
        let kind = field(&[
            "antplus_device_type",
            "ble_device_type",
            "ant_device_type",
            "device_type",
        ])
        .filter(|_| !creator);
        // The watch lists its own parts too, like the barometer, and these have no product or type.
        let is_sensor = kind
            .as_deref()
            .is_some_and(|kind| SENSOR_KINDS.contains(&kind));
        if product.is_none() || !(creator || is_sensor) {
            continue;
        }
        let known = devices
            .iter()
            .any(|device| device.manufacturer == manufacturer && device.product == product);
        if !known {
            devices.push(Device {
                creator,
                manufacturer,
                product,
                kind,
            });
        }
    }
    // The watch first, then the sensors in the order the file lists them.
    devices.sort_by_key(|device| !device.creator);
    Ok(devices)
}
