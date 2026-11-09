use super::{Point, Sample};

/// A GPX 1.1 file of the samples with a position, with elevation, time, and
/// heart rate and cadence in Garmin's extension, which most apps read.
pub fn to_gpx(name: &str, activity_type: &str, samples: &[Sample]) -> String {
    use std::fmt::Write;

    let mut gpx = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n",
        r#"<gpx version="1.1" creator="Firn" xmlns="http://www.topografix.com/GPX/1/1" "#,
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
