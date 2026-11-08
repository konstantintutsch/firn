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

/// The FIT, GPX or TCX file inside a Garmin download, with its name in lower case.
pub fn original_file(zip: &[u8]) -> Result<(String, Vec<u8>)> {
    let mut archive = ZipArchive::new(Cursor::new(zip)).context("malformed ZIP")?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).context("malformed ZIP entry")?;
        let name = file
            .name()
            .context("malformed ZIP entry name")?
            .to_ascii_lowercase();
        if parser_for(&name).is_none() {
            continue;
        }
        let mut content = Vec::new();
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut content)
            .context("could not extract ZIP entry")?;
        if content.len() as u64 > MAX_FILE_BYTES {
            bail!("{name} is too large");
        }
        return Ok((name, content));
    }
    bail!("no FIT, GPX or TCX file in the archive")
}

/// The VO2 max that a device recorded in an activity's FIT file.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vo2Max {
    pub running: Option<f64>,
    pub cycling: Option<f64>,
}

/// The VO2 max in the FIT file of a Garmin download. Other formats have none.
pub fn vo2_max(zip: &[u8]) -> Result<Vo2Max> {
    let (name, content) = original_file(zip)?;
    if !name.ends_with(".fit") {
        return Ok(Vo2Max::default());
    }
    let mut found = Vo2Max::default();
    for record in fitparser::from_bytes(&content).context("malformed FIT")? {
        if record.kind() != MesgNum::MaxMetData {
            continue;
        }
        let (mut value, mut cycling) = (None, false);
        for field in record.fields() {
            match field.name() {
                "vo2_max" => value = number(field.value()),
                "max_met_category" => cycling = field.value().to_string() == "cycling",
                _ => {}
            }
        }
        // Devices write 0 where they have no estimate yet.
        let Some(value) = value.filter(|value| (5.0..=100.0).contains(value)) else {
            continue;
        };
        if cycling {
            found.cycling = Some(value);
        } else {
            found.running = Some(value);
        }
    }
    Ok(found)
}

/// A GPX 1.1 file of the samples with a position, with elevation, time, and
/// heart rate and cadence in Garmin's extension, which most apps read.
pub fn to_gpx(name: &str, activity_type: &str, samples: &[Sample]) -> String {
    use std::fmt::Write;

    let mut gpx = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n",
        r#"<gpx version="1.1" creator="Runmap" xmlns="http://www.topografix.com/GPX/1/1" "#,
        r#"xmlns:gpxtpx="http://www.garmin.com/xmlschemas/TrackPointExtension/v1">"#,
        "\n",
    ));
    let _ = writeln!(
        gpx,
        "  <trk>\n    <name>{}</name>\n    <type>{}</type>\n    <trkseg>",
        escape(name),
        escape(activity_type)
    );
    for sample in samples {
        let Some(Point { lat, lon }) = sample.position else {
            continue;
        };
        let _ = write!(gpx, r#"      <trkpt lat="{lat}" lon="{lon}">"#);
        if let Some(elevation) = sample.elevation_m {
            let _ = write!(gpx, "<ele>{elevation:.1}</ele>");
        }
        if let Some(time) = sample
            .timestamp
            .and_then(|seconds| chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64))
        {
            let _ = write!(
                gpx,
                "<time>{}</time>",
                time.format("%Y-%m-%dT%H:%M:%S%.3fZ")
            );
        }
        if sample.heart_rate.is_some() || sample.cadence.is_some() {
            gpx.push_str("<extensions><gpxtpx:TrackPointExtension>");
            if let Some(heart_rate) = sample.heart_rate {
                let _ = write!(gpx, "<gpxtpx:hr>{}</gpxtpx:hr>", heart_rate.round());
            }
            if let Some(cadence) = sample.cadence {
                let _ = write!(gpx, "<gpxtpx:cad>{}</gpxtpx:cad>", cadence.round());
            }
            gpx.push_str("</gpxtpx:TrackPointExtension></extensions>");
        }
        gpx.push_str("</trkpt>\n");
    }
    gpx.push_str("    </trkseg>\n  </trk>\n</gpx>\n");
    gpx
}

fn escape(text: &str) -> String {
    quick_xml::escape::escape(text).into_owned()
}

type Parser = fn(&[u8]) -> Result<Vec<Sample>>;

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

/// Latitude and longitude, while the element that holds them is read.
type PartialPosition = (Option<f64>, Option<f64>);

/// The field that the text inside a GPX or TCX element belongs to.
#[derive(Clone, Copy)]
enum Field {
    Lat,
    Lon,
    Elevation,
    Time,
    HeartRate,
    Cadence,
    Distance,
    Power,
}

impl Field {
    /// Elements by their local name, which leaves out namespaces like `gpxtpx:hr`.
    fn of(name: &[u8], in_heart_rate: bool) -> Option<Self> {
        Some(match name {
            b"LatitudeDegrees" => Field::Lat,
            b"LongitudeDegrees" => Field::Lon,
            b"ele" | b"AltitudeMeters" => Field::Elevation,
            b"time" | b"Time" => Field::Time,
            b"hr" => Field::HeartRate,
            // In TCX, the heart rate is the Value inside HeartRateBpm.
            b"Value" if in_heart_rate => Field::HeartRate,
            b"cad" | b"Cadence" | b"RunCadence" => Field::Cadence,
            b"DistanceMeters" => Field::Distance,
            b"power" | b"Watts" => Field::Power,
            _ => return None,
        })
    }

    fn set(self, sample: &mut Sample, position: &mut PartialPosition, text: &[u8]) -> Result<()> {
        let text = std::str::from_utf8(text).context("non UTF-8 value")?.trim();
        let number = || text.parse::<f64>().ok().filter(|number| number.is_finite());
        match self {
            Field::Lat => position.0 = Some(parse_degrees(text.as_bytes())?),
            Field::Lon => position.1 = Some(parse_degrees(text.as_bytes())?),
            Field::Elevation => sample.elevation_m = number(),
            Field::Time => {
                sample.timestamp = chrono::DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|time| time.timestamp_millis() as f64 / 1000.0);
            }
            Field::HeartRate => sample.heart_rate = number(),
            Field::Cadence => sample.cadence = number(),
            Field::Distance => sample.distance_m = number(),
            Field::Power => sample.power_w = number(),
        }
        Ok(())
    }
}

/// Reads `trkpt` elements of GPX, or `Trackpoint` elements of TCX, with the
/// values inside them. `point` names the element and `has_attributes` says
/// whether it carries `lat` and `lon` itself, as in GPX.
fn parse_xml(content: &[u8], point: &[u8], has_attributes: bool) -> Result<Vec<Sample>> {
    let mut reader = Reader::from_reader(content);
    let mut buffer = Vec::new();
    let mut samples = Vec::new();
    let mut current: Option<(Sample, PartialPosition)> = None;
    let mut field = None;
    let mut in_heart_rate = false;

    let finish = |sample: Sample, (lat, lon): PartialPosition| -> Result<Sample> {
        let position = match (lat, lon) {
            (Some(lat), Some(lon)) => Some(checked_point(lat, lon)?),
            (None, None) if !has_attributes => None,
            _ => bail!("track point without coordinates"),
        };
        Ok(Sample { position, ..sample })
    };
    let attributes = |element: &quick_xml::events::BytesStart| -> Result<PartialPosition> {
        let (mut lat, mut lon) = (None, None);
        for attribute in element.attributes() {
            let attribute = attribute.context("malformed attribute")?;
            match attribute.key.as_ref() {
                b"lat" => lat = Some(parse_degrees(&attribute.value)?),
                b"lon" => lon = Some(parse_degrees(&attribute.value)?),
                _ => {}
            }
        }
        Ok((lat, lon))
    };

    loop {
        match reader
            .read_event_into(&mut buffer)
            .context("malformed XML")?
        {
            Event::Empty(element) if element.local_name().as_ref() == point => {
                let position = if has_attributes {
                    attributes(&element)?
                } else {
                    (None, None)
                };
                samples.push(finish(Sample::default(), position)?);
            }
            Event::Start(element) => {
                let name = element.local_name();
                if name.as_ref() == point {
                    let position = if has_attributes {
                        attributes(&element)?
                    } else {
                        (None, None)
                    };
                    current = Some((Sample::default(), position));
                } else if name.as_ref() == b"HeartRateBpm" {
                    in_heart_rate = true;
                } else if current.is_some() {
                    field = Field::of(name.as_ref(), in_heart_rate);
                }
            }
            Event::Text(text) => {
                if let (Some(field), Some((sample, position))) = (field, &mut current) {
                    field.set(sample, position, &text)?;
                }
            }
            Event::End(element) => {
                field = None;
                let name = element.local_name();
                if name.as_ref() == b"HeartRateBpm" {
                    in_heart_rate = false;
                } else if name.as_ref() == point
                    && let Some((sample, position)) = current.take()
                {
                    samples.push(finish(sample, position)?);
                }
            }
            Event::Eof => return Ok(samples),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_gpx(content: &[u8]) -> Result<Vec<Sample>> {
    parse_xml(content, b"trkpt", true).context("malformed GPX")
}

fn parse_tcx(content: &[u8]) -> Result<Vec<Sample>> {
    parse_xml(content, b"Trackpoint", false).context("malformed TCX")
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
