use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

use super::{Point, Sample};

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

pub(super) fn parse_gpx(content: &[u8]) -> Result<Vec<Sample>> {
    parse_xml(content, b"trkpt", true).context("malformed GPX")
}

/// The points of a GPX file for a route: its track, or its route points if it has no track.
pub fn parse_gpx_route(content: &[u8]) -> Result<Vec<Sample>> {
    let mut samples = parse_gpx(content)?;
    if !samples.iter().any(|sample| sample.position.is_some()) {
        samples = parse_xml(content, b"rtept", true).context("malformed GPX")?;
    }
    Ok(samples)
}

pub(super) fn parse_tcx(content: &[u8]) -> Result<Vec<Sample>> {
    parse_xml(content, b"Trackpoint", false).context("malformed TCX")
}

pub(super) fn parse_degrees(value: &[u8]) -> Result<f64> {
    std::str::from_utf8(value)
        .context("non UTF-8 coordinate")?
        .trim()
        .parse()
        .context("invalid coordinate")
}

pub(super) fn checked_point(lat: f64, lon: f64) -> Result<Point> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        bail!("track point out of range: {lat}, {lon}");
    }
    Ok(Point { lat, lon })
}
