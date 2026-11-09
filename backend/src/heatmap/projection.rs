use std::f64::consts::PI;

pub type Position = [f64; 2];
pub type Track = Vec<Position>;

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub min: Position,
    pub max: Position,
}

impl Viewport {
    /// Whether the two overlap or touch.
    pub fn intersects(&self, other: &Viewport) -> bool {
        self.min[0] <= other.max[0]
            && other.min[0] <= self.max[0]
            && self.min[1] <= other.max[1]
            && other.min[1] <= self.max[1]
    }

    pub fn from_degrees(west: f64, south: f64, east: f64, north: f64) -> Self {
        // Mercator y grows southward, so the viewport's smallest y is its north edge.
        Self {
            min: project(north, west),
            max: project(south, east),
        }
    }
}

pub fn project(lat: f64, lon: f64) -> Position {
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    [
        (lon + 180.0) / 360.0,
        (1.0 - (lat / 2.0 + PI / 4.0).tan().ln() / PI) / 2.0,
    ]
}

/// The smallest viewport around a track, or `None` if it has no points.
pub fn bounds(track: &[Position]) -> Option<Viewport> {
    let first = *track.first()?;
    Some(track.iter().fold(
        Viewport {
            min: first,
            max: first,
        },
        |bounds, position| Viewport {
            min: [
                bounds.min[0].min(position[0]),
                bounds.min[1].min(position[1]),
            ],
            max: [
                bounds.max[0].max(position[0]),
                bounds.max[1].max(position[1]),
            ],
        },
    ))
}

/// Returns longitude and latitude in degrees.
pub fn unproject([x, y]: Position) -> Position {
    [
        x * 360.0 - 180.0,
        (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees(),
    ]
}
