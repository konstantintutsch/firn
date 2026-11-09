mod device;
mod fit;
mod gpx;
mod xml;

pub use device::{Device, devices};
pub use fit::vo2_max;
pub use gpx::to_gpx;
pub use xml::parse_gpx_route;

use std::io::{Cursor, Read};

use anyhow::{Context, Result, bail};
use zip::ZipArchive;

use fit::parse_fit;
use xml::{parse_gpx, parse_tcx};

/// Far above any real activity file, but protects against a corrupt archive.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

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

/// How many points of a track the small picture of an activity has.
const PREVIEW_POINTS: usize = 64;

/// At most 64 points of a track, evenly spread and with the first and the last,
/// as `[latitude, longitude]`.
pub fn preview(points: &[Point]) -> Vec<[f64; 2]> {
    let last = points.len().saturating_sub(1);
    let count = points.len().min(PREVIEW_POINTS);
    (0..count)
        .map(|index| {
            let point = if count < 2 {
                points[0]
            } else {
                points[index * last / (count - 1)]
            };
            [point.lat, point.lon]
        })
        .collect()
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
