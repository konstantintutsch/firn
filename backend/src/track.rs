use std::io::{Cursor, Read};

use anyhow::{Context, Result, bail};
use fitparser::Value;
use fitparser::profile::MesgNum;
use quick_xml::Reader;
use quick_xml::events::Event;
use zip::ZipArchive;

/// Far above any real activity file, but protects against a corrupt archive.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// FIT stores coordinates as 32-bit integers, where 2^31 semicircles are 180 degrees.
const DEGREES_PER_SEMICIRCLE: f64 = 180.0 / 2_147_483_648.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
}

/// One recorded moment of an activity. Only FIT files record more than the position.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    /// `None` without a GPS fix.
    pub position: Option<Point>,
    /// Seconds since 1970.
    pub timestamp: Option<f64>,
    /// Since the start.
    pub distance_m: Option<f64>,
    pub elevation_m: Option<f64>,
    pub heart_rate: Option<f64>,
    pub speed_m_s: Option<f64>,
    /// As the device records it: revolutions, or strides of one foot, per minute.
    pub cadence: Option<f64>,
    pub power_w: Option<f64>,
    pub temperature_c: Option<f64>,
}

/// Extracts the track from a Garmin download: a ZIP with the original FIT file,
/// or GPX or TCX for activities that were uploaded in those formats.
pub fn parse_archive(zip: &[u8]) -> Result<Vec<Point>> {
    Ok(parse_samples(zip)?
        .into_iter()
        .filter_map(|sample| sample.position)
        .collect())
}

/// Every sample of a Garmin download, with or without a position.
pub fn parse_samples(zip: &[u8]) -> Result<Vec<Sample>> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).context("malformed ZIP")?;
    let mut points = Vec::new();
    let mut found_track_file = false;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).context("malformed ZIP entry")?;
        let name = file
            .name()
            .context("malformed ZIP entry name")?
            .to_ascii_lowercase();
        let Some(parse) = parser_for(&name) else {
            continue;
        };
        found_track_file = true;

        let mut content = Vec::new();
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut content)
            .context("could not extract ZIP entry")?;
        if content.len() as u64 > MAX_FILE_BYTES {
            bail!("{name} is too large");
        }
        points.extend(parse(&content).with_context(|| format!("could not parse {name}"))?);
    }

    if !found_track_file {
        bail!("no FIT, GPX or TCX file in the archive");
    }
    // Devices without a GPS fix can record 0, 0, which is in the ocean off Africa.
    for sample in &mut points {
        if sample.position == Some(Point { lat: 0.0, lon: 0.0 }) {
            sample.position = None;
        }
    }
    Ok(points)
}

type Parser = fn(&[u8]) -> Result<Vec<Sample>>;

fn located(point: Point) -> Sample {
    Sample {
        position: Some(point),
        ..Sample::default()
    }
}

fn parser_for(name: &str) -> Option<Parser> {
    match name.rsplit_once('.')?.1 {
        "fit" => Some(parse_fit),
        "gpx" => Some(parse_gpx),
        "tcx" => Some(parse_tcx),
        _ => None,
    }
}

fn parse_fit(content: &[u8]) -> Result<Vec<Sample>> {
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

fn number(value: &Value) -> Option<f64> {
    value
        .clone()
        .try_into()
        .ok()
        .filter(|number: &f64| number.is_finite())
}

fn parse_gpx(content: &[u8]) -> Result<Vec<Sample>> {
    let mut reader = Reader::from_reader(content);
    let mut buffer = Vec::new();
    let mut points = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buffer)
            .context("malformed GPX")?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"trkpt" =>
            {
                let (mut lat, mut lon) = (None, None);
                for attribute in element.attributes() {
                    let attribute = attribute.context("malformed GPX attribute")?;
                    match attribute.key.as_ref() {
                        b"lat" => lat = Some(parse_degrees(&attribute.value)?),
                        b"lon" => lon = Some(parse_degrees(&attribute.value)?),
                        _ => {}
                    }
                }
                let (Some(lat), Some(lon)) = (lat, lon) else {
                    bail!("track point without coordinates");
                };
                points.push(located(checked_point(lat, lon)?));
            }
            Event::Eof => return Ok(points),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_tcx(content: &[u8]) -> Result<Vec<Sample>> {
    #[derive(Clone, Copy)]
    enum Coordinate {
        Lat,
        Lon,
    }

    let mut reader = Reader::from_reader(content);
    let mut buffer = Vec::new();
    let mut points = Vec::new();
    let mut current = None;
    let (mut lat, mut lon) = (None, None);

    loop {
        match reader
            .read_event_into(&mut buffer)
            .context("malformed TCX")?
        {
            Event::Start(element) => match element.local_name().as_ref() {
                b"LatitudeDegrees" => current = Some(Coordinate::Lat),
                b"LongitudeDegrees" => current = Some(Coordinate::Lon),
                b"Position" => (lat, lon) = (None, None),
                _ => {}
            },
            Event::Text(text) => match current {
                Some(Coordinate::Lat) => lat = Some(parse_degrees(&text)?),
                Some(Coordinate::Lon) => lon = Some(parse_degrees(&text)?),
                None => {}
            },
            Event::End(element) => {
                current = None;
                if element.local_name().as_ref() == b"Position" {
                    let (Some(lat), Some(lon)) = (lat, lon) else {
                        bail!("position without coordinates");
                    };
                    points.push(located(checked_point(lat, lon)?));
                }
            }
            Event::Eof => return Ok(points),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_degrees(value: &[u8]) -> Result<f64> {
    std::str::from_utf8(value)
        .context("non UTF-8 coordinate")?
        .trim()
        .parse()
        .context("invalid coordinate")
}

fn checked_point(lat: f64, lon: f64) -> Result<Point> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        bail!("track point out of range: {lat}, {lon}");
    }
    Ok(Point { lat, lon })
}
