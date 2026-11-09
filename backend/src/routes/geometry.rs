use crate::api::ApiError;
use crate::track::{Point, Sample};

/// Enough to follow a route on a map, and few enough to keep the answer small.
const MAX_POINTS: usize = 5000;
/// Points of the small picture of a route.
const PREVIEW_POINTS: usize = 64;
/// Smaller changes of elevation count as noise, which would otherwise add up to a climb.
const ELEVATION_NOISE_M: f64 = 3.0;
const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// A point of a route: latitude, longitude and elevation in meters, if known.
pub(super) type RoutePoint = (f64, f64, Option<f64>);

/// Meters between two points along the Earth's surface.
fn haversine(a: &RoutePoint, b: &RoutePoint) -> f64 {
    let (lat_a, lat_b) = (a.0.to_radians(), b.0.to_radians());
    let across = ((b.1 - a.1).to_radians() / 2.0).sin();
    let along = ((lat_b - lat_a) / 2.0).sin();
    let h = along * along + lat_a.cos() * lat_b.cos() * across * across;
    2.0 * EARTH_RADIUS_M * h.sqrt().asin()
}

fn distance(points: &[RoutePoint]) -> f64 {
    points
        .windows(2)
        .map(|pair| haversine(&pair[0], &pair[1]))
        .sum()
}

/// The meters climbed, ignoring changes smaller than the noise of a barometer or GPS.
/// `None` if no point has an elevation.
fn elevation_gain(points: &[RoutePoint]) -> Option<f64> {
    let mut elevations = points.iter().filter_map(|point| point.2);
    let mut reference = elevations.next()?;
    let mut gain = 0.0;
    for elevation in elevations {
        if (elevation - reference).abs() >= ELEVATION_NOISE_M {
            gain += (elevation - reference).max(0.0);
            reference = elevation;
        }
    }
    Some(gain)
}

/// At most `max` points, evenly spread, always with the first and the last.
fn spread(points: &[RoutePoint], max: usize) -> Vec<RoutePoint> {
    if points.len() <= max {
        return points.to_vec();
    }
    let last = points.len() - 1;
    (0..max)
        .map(|index| points[index * last / (max - 1)])
        .collect()
}

/// What a route is saved with, worked out from its points.
pub struct Prepared {
    pub points: Vec<RoutePoint>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    /// `[latitude, longitude]` of a few points.
    pub preview: Vec<[f64; 2]>,
}

/// Checks the points of a route that came from outside, and thins them.
pub fn prepare(points: Vec<RoutePoint>) -> Result<Prepared, ApiError> {
    if points.len() < 2 {
        return Err(ApiError::BadRequest("the track has fewer than two points"));
    }
    if points
        .iter()
        .any(|(lat, lon, _)| !(-90.0..=90.0).contains(lat) || !(-180.0..=180.0).contains(lon))
    {
        return Err(ApiError::BadRequest("a point is out of range"));
    }
    let points = spread(&points, MAX_POINTS);
    Ok(Prepared {
        distance_m: distance(&points),
        elevation_gain_m: elevation_gain(&points),
        preview: spread(&points, PREVIEW_POINTS)
            .into_iter()
            .map(|(lat, lon, _)| [lat, lon])
            .collect(),
        points,
    })
}

/// The samples with a position, as route points.
pub(super) fn route_points(samples: &[Sample]) -> Result<Vec<RoutePoint>, ApiError> {
    let points: Vec<_> = samples
        .iter()
        .filter_map(|sample| {
            let Point { lat, lon } = sample.position?;
            Some((
                lat,
                lon,
                sample.elevation_m.filter(|value| value.is_finite()),
            ))
        })
        .collect();
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_degree_of_latitude() {
        let meters = distance(&[(0.0, 0.0, None), (1.0, 0.0, None)]);
        assert!((meters - 111_195.0).abs() < 10.0, "{meters}");
    }

    #[test]
    fn climbs_ignore_noise() {
        let points: Vec<RoutePoint> = [100.0, 101.0, 99.5, 110.0, 108.0, 120.0]
            .into_iter()
            .map(|elevation| (0.0, 0.0, Some(elevation)))
            .collect();
        // 100 to 110 and 110 to 120 are climbs. The dip to 108 is noise.
        assert_eq!(elevation_gain(&points), Some(20.0));
        assert_eq!(elevation_gain(&[(0.0, 0.0, None)]), None);
    }

    #[test]
    fn thinning_keeps_the_ends() {
        let points: Vec<RoutePoint> = (0..20_000)
            .map(|index| (f64::from(index), 0.0, None))
            .collect();
        let thinned = spread(&points, MAX_POINTS);
        assert_eq!(thinned.len(), MAX_POINTS);
        assert_eq!(thinned[0].0, 0.0);
        assert_eq!(thinned[MAX_POINTS - 1].0, 19_999.0);
    }
}
