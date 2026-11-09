use anyhow::{Context, Result};
use fitparser::Value;
use fitparser::profile::MesgNum;

use super::xml::checked_point;
use super::{Sample, original_file};

/// FIT stores coordinates as 32-bit integers, where 2^31 semicircles are 180 degrees.
const DEGREES_PER_SEMICIRCLE: f64 = 180.0 / 2_147_483_648.0;

/// Garmin's activity metrics, which the FIT profile does not name.
const ACTIVITY_METRICS_MESSAGE: u16 = 140;
/// The field of the activity metrics that holds VO2 max, in 1/65536 of a MET.
const VO2_MAX_FIELD: &str = "unknown_field_7";
/// One MET is 3.5 mL of oxygen per kg and minute.
const ML_PER_KG_MIN_PER_RAW: f64 = 3.5 / 65536.0;

/// The VO2 max that a device recorded in an activity's FIT file. Other formats have none.
pub fn vo2_max(zip: &[u8]) -> Result<Option<f64>> {
    let (name, content) = original_file(zip)?;
    if !name.ends_with(".fit") {
        return Ok(None);
    }
    let mut found = None;
    for record in fitparser::from_bytes(&content).context("malformed FIT")? {
        if record.kind().as_u16() != ACTIVITY_METRICS_MESSAGE {
            continue;
        }
        let value = record
            .fields()
            .iter()
            .find(|field| field.name() == VO2_MAX_FIELD)
            .and_then(|field| number(field.value()))
            .map(|raw| raw * ML_PER_KG_MIN_PER_RAW);
        // Devices write 0 where they have no estimate yet.
        if let Some(value) = value.filter(|value| (5.0..=100.0).contains(value)) {
            found = Some(value);
        }
    }
    Ok(found)
}

pub(super) fn parse_fit(content: &[u8]) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    for record in fitparser::from_bytes(content).context("malformed FIT")? {
        if record.kind() != MesgNum::Record {
            continue;
        }
        let mut sample = Sample::default();
        let (mut lat, mut lon) = (None, None);
        let (mut altitude, mut enhanced_altitude) = (None, None);
        let (mut speed, mut enhanced_speed) = (None, None);
        let mut fractional_cadence = None;
        for field in record.fields() {
            let value = field.value();
            match (field.name(), value) {
                ("position_lat", Value::SInt32(value)) => lat = Some(*value),
                ("position_long", Value::SInt32(value)) => lon = Some(*value),
                ("timestamp", Value::Timestamp(time)) => {
                    sample.timestamp = Some(time.timestamp() as f64);
                }
                ("altitude", _) => altitude = number(value),
                ("enhanced_altitude", _) => enhanced_altitude = number(value),
                ("speed", _) => speed = number(value),
                ("enhanced_speed", _) => enhanced_speed = number(value),
                ("distance", _) => sample.distance_m = number(value),
                ("heart_rate", _) => sample.heart_rate = number(value),
                ("cadence", _) => sample.cadence = number(value),
                ("fractional_cadence", _) => fractional_cadence = number(value),
                ("power", _) => sample.power_w = number(value),
                ("temperature", _) => sample.temperature_c = number(value),
                _ => {}
            }
        }
        // Newer devices record the enhanced fields, which have a larger range.
        sample.elevation_m = enhanced_altitude.or(altitude);
        sample.speed_m_s = enhanced_speed.or(speed);
        if let (Some(cadence), Some(fraction)) = (sample.cadence, fractional_cadence) {
            sample.cadence = Some(cadence + fraction);
        }
        // Records without a GPS fix leave the position out or mark it as invalid.
        if let (Some(lat), Some(lon)) = (lat, lon)
            && lat != i32::MAX
            && lon != i32::MAX
        {
            sample.position = Some(checked_point(
                f64::from(lat) * DEGREES_PER_SEMICIRCLE,
                f64::from(lon) * DEGREES_PER_SEMICIRCLE,
            )?);
        }
        samples.push(sample);
    }
    Ok(samples)
}

pub(super) fn number(value: &Value) -> Option<f64> {
    value
        .clone()
        .try_into()
        .ok()
        .filter(|number: &f64| number.is_finite())
}
