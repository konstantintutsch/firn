use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
}

/// Extracts every track point from a GPX document, in file order.
pub fn parse_track(gpx: &str) -> Result<Vec<Point>> {
    let mut reader = Reader::from_str(gpx);
    let mut points = Vec::new();

    loop {
        match reader.read_event().context("malformed GPX")? {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"trkpt" =>
            {
                let (mut lat, mut lon) = (None, None);
                for attribute in element.attributes() {
                    let attribute = attribute.context("malformed GPX attribute")?;
                    let value = std::str::from_utf8(&attribute.value)
                        .context("non UTF-8 coordinate")?
                        .parse::<f64>();
                    match attribute.key.as_ref() {
                        b"lat" => lat = Some(value.context("invalid latitude")?),
                        b"lon" => lon = Some(value.context("invalid longitude")?),
                        _ => {}
                    }
                }
                let (Some(lat), Some(lon)) = (lat, lon) else {
                    bail!("track point without coordinates");
                };
                if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
                    bail!("track point out of range: {lat}, {lon}");
                }
                points.push(Point { lat, lon });
            }
            Event::Eof => return Ok(points),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_points_across_segments() {
        let gpx = r#"<?xml version="1.0"?>
            <gpx xmlns="http://www.topografix.com/GPX/1/1"><trk>
              <trkseg><trkpt lat="52.5" lon="13.4"><ele>30</ele></trkpt></trkseg>
              <trkseg><trkpt lon="-0.12" lat="51.5"/></trkseg>
            </trk></gpx>"#;
        assert_eq!(
            parse_track(gpx).unwrap(),
            [
                Point {
                    lat: 52.5,
                    lon: 13.4
                },
                Point {
                    lat: 51.5,
                    lon: -0.12
                }
            ]
        );
    }

    #[test]
    fn empty_track_has_no_points() {
        assert!(parse_track("<gpx><trk/></gpx>").unwrap().is_empty());
    }

    #[test]
    fn rejects_out_of_range_coordinates() {
        assert!(parse_track(r#"<gpx><trkpt lat="91" lon="0"/></gpx>"#).is_err());
    }

    #[test]
    fn rejects_truncated_points() {
        assert!(parse_track(r#"<gpx><trkpt lat="1"/></gpx>"#).is_err());
    }
}
