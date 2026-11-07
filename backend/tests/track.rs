mod common;

use common::{fit_of, semicircles, zip_of};
use runmap::track::{Point, parse_archive};

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
