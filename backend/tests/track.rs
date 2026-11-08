mod common;

use common::{
    FitRecord, fit_of, fit_of_devices, fit_of_records, fit_of_vo2_max, semicircles, zip_of,
};
use runmap::track::{Point, devices, original_file, parse_archive, parse_samples, to_gpx, vo2_max};

const GPX: &[u8] = br#"<?xml version="1.0"?>
    <gpx xmlns="http://www.topografix.com/GPX/1/1"><trk>
      <trkseg><trkpt lat="52.5" lon="13.4"><ele>30</ele></trkpt></trkseg>
      <trkseg><trkpt lon="-0.12" lat="51.5"/></trkseg>
    </trk></gpx>"#;
const TCX: &[u8] = br#"<TrainingCenterDatabase><Activities><Activity><Lap><Track>
      <Trackpoint><Time>2026-01-01T00:00:00Z</Time></Trackpoint>
      <Trackpoint><Position>
        <LatitudeDegrees>52.5</LatitudeDegrees><LongitudeDegrees>13.4</LongitudeDegrees>
      </Position></Trackpoint>
    </Track></Lap></Activity></Activities></TrainingCenterDatabase>"#;

fn assert_close(actual: Point, lat: f64, lon: f64) {
    assert!((actual.lat - lat).abs() < 1e-6, "{actual:?}");
    assert!((actual.lon - lon).abs() < 1e-6, "{actual:?}");
}

#[test]
fn reads_fit_positions_in_degrees() {
    let fit = fit_of(&[
        (semicircles(52.5), semicircles(13.4)),
        (semicircles(-33.87), semicircles(151.21)),
    ]);
    let points = parse_archive(&zip_of(&[("123_ACTIVITY.fit", &fit)])).unwrap();
    assert_eq!(points.len(), 2);
    assert_close(points[0], 52.5, 13.4);
    assert_close(points[1], -33.87, 151.21);
}

#[test]
fn skips_fit_records_without_a_gps_fix() {
    let fit = fit_of(&[
        (i32::MAX, i32::MAX),
        (0, 0),
        (semicircles(52.5), semicircles(13.4)),
    ]);
    let points = parse_archive(&zip_of(&[("a.FIT", &fit)])).unwrap();
    assert_eq!(points.len(), 1);
    assert_close(points[0], 52.5, 13.4);
}

#[test]
fn fit_without_positions_is_an_empty_track() {
    let points = parse_archive(&zip_of(&[("a.fit", &fit_of(&[]))])).unwrap();
    assert!(points.is_empty());
}

#[test]
fn reads_gpx_points_across_segments() {
    let points = parse_archive(&zip_of(&[("a.gpx", GPX)])).unwrap();
    assert_eq!(
        points,
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
fn reads_tcx_positions() {
    let points = parse_archive(&zip_of(&[("a.tcx", TCX)])).unwrap();
    assert_eq!(
        points,
        [Point {
            lat: 52.5,
            lon: 13.4
        }]
    );
}

#[test]
fn reads_gpx_and_tcx_metrics() {
    let gpx = br#"<gpx><trk><trkseg>
      <trkpt lat="52.5" lon="13.4"><ele>30.5</ele><time>2026-01-01T08:00:00Z</time>
        <extensions><gpxtpx:TrackPointExtension>
          <gpxtpx:hr>140</gpxtpx:hr><gpxtpx:cad>85</gpxtpx:cad>
        </gpxtpx:TrackPointExtension></extensions>
      </trkpt>
    </trkseg></trk></gpx>"#;
    let samples = parse_samples(&zip_of(&[("a.gpx", gpx)])).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].elevation_m, Some(30.5));
    assert_eq!(samples[0].timestamp, Some(1_767_254_400.0));
    assert_eq!(samples[0].heart_rate, Some(140.0));
    assert_eq!(samples[0].cadence, Some(85.0));

    let tcx = br#"<TrainingCenterDatabase><Activities><Activity><Lap><Track>
      <Trackpoint><Time>2026-01-01T08:00:05Z</Time>
        <Position><LatitudeDegrees>52.5</LatitudeDegrees><LongitudeDegrees>13.4</LongitudeDegrees></Position>
        <AltitudeMeters>31</AltitudeMeters><DistanceMeters>12.5</DistanceMeters>
        <HeartRateBpm><Value>141</Value></HeartRateBpm>
      </Trackpoint>
    </Track></Lap></Activity></Activities></TrainingCenterDatabase>"#;
    let samples = parse_samples(&zip_of(&[("a.tcx", tcx)])).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].timestamp, Some(1_767_254_405.0));
    assert_eq!(samples[0].distance_m, Some(12.5));
    assert_eq!(samples[0].heart_rate, Some(141.0));
    assert_eq!(samples[0].elevation_m, Some(31.0));
}

#[test]
fn ignores_other_files_in_the_archive() {
    let fit = fit_of(&[(semicircles(1.0), semicircles(2.0))]);
    let points = parse_archive(&zip_of(&[("notes.txt", b"hi"), ("a.fit", &fit)])).unwrap();
    assert_eq!(points.len(), 1);
}

#[test]
fn rejects_unusable_downloads() {
    assert!(parse_archive(b"not a zip").is_err());
    assert!(parse_archive(&zip_of(&[("notes.txt", b"hi")])).is_err());
    assert!(parse_archive(&zip_of(&[("a.fit", b"not a fit file")])).is_err());
    assert!(parse_archive(&zip_of(&[("a.gpx", br#"<trkpt lat="91" lon="0"/>"#)])).is_err());
    assert!(parse_archive(&zip_of(&[("a.gpx", br#"<trkpt lat="1"/>"#)])).is_err());
}

#[test]
fn reads_fit_metrics_in_their_units() {
    let fit = fit_of_records(&[
        FitRecord {
            lat: semicircles(52.5),
            lon: semicircles(13.4),
            timestamp: 1_000_000_000,
            altitude: (34 + 500) * 5,
            heart_rate: 150,
            distance: 12_345,
            speed: 3_250,
        },
        FitRecord {
            lat: i32::MAX,
            lon: i32::MAX,
            timestamp: 1_000_000_010,
            altitude: (36 + 500) * 5,
            heart_rate: 152,
            distance: 15_000,
            speed: 3_000,
        },
    ]);
    let samples = parse_samples(&zip_of(&[("a.fit", &fit)])).unwrap();
    assert_eq!(samples.len(), 2);
    let [first, second] = [samples[0], samples[1]];
    assert_close(first.position.unwrap(), 52.5, 13.4);
    assert_eq!(first.elevation_m, Some(34.0));
    assert_eq!(first.heart_rate, Some(150.0));
    assert_eq!(first.distance_m, Some(123.45));
    assert_eq!(first.speed_m_s, Some(3.25));
    assert_eq!(first.power_w, None);
    // FIT counts seconds from 1989-12-31.
    assert_eq!(first.timestamp, Some(631_065_600.0 + 1_000_000_000.0));
    assert_eq!(second.timestamp.unwrap() - first.timestamp.unwrap(), 10.0);
    assert_eq!(second.position, None);
    assert_eq!(second.elevation_m, Some(36.0));

    // The track keeps only the samples with a position.
    assert_eq!(parse_archive(&zip_of(&[("a.fit", &fit)])).unwrap().len(), 1);
}

#[test]
fn gpx_export_reads_back_the_same_samples() {
    let gpx = br#"<gpx><trk><trkseg>
      <trkpt lat="52.5" lon="13.4"><ele>30.5</ele><time>2026-01-01T08:00:00Z</time>
        <extensions><gpxtpx:TrackPointExtension><gpxtpx:hr>140</gpxtpx:hr></gpxtpx:TrackPointExtension></extensions>
      </trkpt>
      <trkpt lat="52.6" lon="13.5"/>
    </trkseg></trk></gpx>"#;
    let samples = parse_samples(&zip_of(&[("a.gpx", gpx)])).unwrap();
    let exported = to_gpx("Run <& friends>", "running", &samples);
    assert!(exported.contains("<name>Run &lt;&amp; friends&gt;</name>"));
    let again = parse_samples(&zip_of(&[("b.gpx", exported.as_bytes())])).unwrap();
    assert_eq!(again, samples);
}

#[test]
fn finds_the_original_file_in_the_archive() {
    let fit = fit_of(&[(semicircles(1.0), semicircles(2.0))]);
    let (name, content) =
        original_file(&zip_of(&[("notes.txt", b"hi"), ("123_ACTIVITY.FIT", &fit)])).unwrap();
    assert_eq!(name, "123_activity.fit");
    assert_eq!(content, fit);
    assert!(original_file(&zip_of(&[("notes.txt", b"hi")])).is_err());
}

#[test]
fn reads_vo2_max_from_the_activity_metrics_of_fit() {
    // A raw 0 is no estimate yet.
    let fit = fit_of_vo2_max(&[0, 926_115]);
    let found = vo2_max(&zip_of(&[("a.fit", &fit)])).unwrap().unwrap();
    assert!((found - 49.4599).abs() < 1e-4, "{found}");
}

#[test]
fn fit_and_gpx_without_vo2_max_have_none() {
    let fit = vo2_max(&zip_of(&[("a.fit", &fit_of(&[]))])).unwrap();
    assert_eq!(fit, None);
    let gpx = vo2_max(&zip_of(&[("a.gpx", GPX)])).unwrap();
    assert_eq!(gpx, None);
}

#[test]
fn reads_the_watch_and_the_sensors_of_fit() {
    // The watch, its barometer (a local part with the watch's product), a heart rate strap
    // over ANT+ (type 120), another over Bluetooth (type 1), the strap again with a
    // different type, and the watch again, as the file repeats them.
    let fit = fit_of_devices(&[
        (0, 0, 5, 1, 4024),
        (1, 0, 5, 1, 4024),
        (2, 120, 1, 1, 2),
        (3, 1, 3, 1, 4607),
        (4, 1, 3, 1, 4607),
        (0, 0, 5, 1, 4024),
    ]);
    let found = devices(&zip_of(&[("a.fit", &fit)])).unwrap();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found[0].creator);
    assert_eq!(found[0].manufacturer.as_deref(), Some("garmin"));
    assert!(found[1..].iter().all(|device| !device.creator));
    assert!(
        found[1..]
            .iter()
            .all(|device| device.kind.as_deref() == Some("heart_rate"))
    );
}

#[test]
fn fit_without_device_info_and_gpx_have_no_devices() {
    assert!(
        devices(&zip_of(&[("a.fit", &fit_of(&[]))]))
            .unwrap()
            .is_empty()
    );
    assert!(devices(&zip_of(&[("a.gpx", GPX)])).unwrap().is_empty());
}
