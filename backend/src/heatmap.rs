//! Turns tracks into colored lines for a map viewport.
//!
//! Positions are Web Mercator coordinates in the unit square.

use std::collections::HashMap;
use std::f64::consts::PI;

/// World width in pixels at zoom 0 for MapLibre's 512 px tiles.
const WORLD_PIXELS: f64 = 512.0;
/// Runs count as passing the same spot if they share a cell this wide.
const COUNT_CELL_PIXELS: f64 = 4.0;
/// About 12 m at the equator. Runs on one street spread over this much GPS error,
/// so smaller cells would split them into seemingly different routes.
const MIN_COUNT_CELL: f64 = 3e-7;
/// Simplifies lines by up to this much, which is too little to see.
const SIMPLIFY_PIXELS: f64 = 0.25;
/// Number of color steps. Fewer steps merge more segments into one line.
const LEVELS: u32 = 8;

pub type Position = [f64; 2];
pub type Track = Vec<Position>;

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub min: Position,
    pub max: Position,
}

impl Viewport {
    pub fn from_degrees(west: f64, south: f64, east: f64, north: f64) -> Self {
        // Mercator y grows southward, so the viewport's smallest y is its north edge.
        Self {
            min: project(north, west),
            max: project(south, east),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct HeatLine {
    /// From 0 for the fewest runs in the viewport to 1 for the most, on a log scale.
    pub weight: f64,
    pub path: Vec<Position>,
}

pub fn project(lat: f64, lon: f64) -> Position {
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    [
        (lon + 180.0) / 360.0,
        (1.0 - (lat / 2.0 + PI / 4.0).tan().ln() / PI) / 2.0,
    ]
}

/// Returns longitude and latitude in degrees.
pub fn unproject([x, y]: Position) -> Position {
    [
        x * 360.0 - 180.0,
        (PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees(),
    ]
}

/// Builds the lines to draw, sorted so that the busiest are last and end up on top.
///
/// Consecutive points always connect, however far apart: a gap usually means lost
/// GPS, as in a tunnel, and the straight line shows that the run went through.
pub fn heat_lines(tracks: &[Track], viewport: &Viewport, zoom: u8) -> Vec<HeatLine> {
    let pixel = 1.0 / (WORLD_PIXELS * 2f64.powi(zoom.into()));
    let cell = (COUNT_CELL_PIXELS * pixel).max(MIN_COUNT_CELL);
    let runs_per_cell = count_runs(tracks, viewport, cell);
    let max_runs = runs_per_cell
        .values()
        .map(|visit| visit.runs)
        .max()
        .unwrap_or(1);
    let level_of = |position: Position| {
        let runs = nearby_runs(&runs_per_cell, cell_of(position, cell));
        let share = f64::from(runs).ln_1p() / f64::from(max_runs).ln_1p();
        (share * f64::from(LEVELS - 1)).round() as u32
    };

    let mut pieces: Vec<(u32, Vec<Position>)> = Vec::new();
    for track in tracks {
        let mut current: Option<(u32, Vec<Position>)> = None;
        for segment in track.windows(2) {
            let Some([a, b]) = clip(segment[0], segment[1], viewport) else {
                pieces.extend(current.take());
                continue;
            };
            let level = level_of(midpoint(a, b));
            match &mut current {
                Some((current_level, path))
                    if *current_level == level && path.last() == Some(&a) =>
                {
                    path.push(b);
                }
                _ => {
                    pieces.extend(current.take());
                    current = Some((level, vec![a, b]));
                }
            }
        }
        pieces.extend(current);
    }

    pieces.sort_by_key(|(level, _)| *level);
    pieces
        .into_iter()
        .map(|(level, path)| HeatLine {
            weight: f64::from(level) / f64::from(LEVELS - 1),
            path: simplify(&path, SIMPLIFY_PIXELS * pixel),
        })
        .collect()
}

struct Visit {
    last_track: usize,
    runs: u32,
}

fn count_runs(tracks: &[Track], viewport: &Viewport, cell: f64) -> HashMap<(i64, i64), Visit> {
    let mut visits = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        for segment in track.windows(2) {
            let Some([a, b]) = clip(segment[0], segment[1], viewport) else {
                continue;
            };
            let steps = (distance(a, b) / cell).ceil().max(1.0);
            for step in 0..=steps as u64 {
                let t = step as f64 / steps;
                let position = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                let visit = visits.entry(cell_of(position, cell)).or_insert(Visit {
                    last_track: index,
                    runs: 1,
                });
                if visit.last_track != index {
                    visit.last_track = index;
                    visit.runs += 1;
                }
            }
        }
    }
    visits
}

/// The busiest of a cell and its neighbors. GPS error scatters a shared route over
/// neighboring cells, and their counts differ by chance, which would make its color
/// flicker along the way.
fn nearby_runs(visits: &HashMap<(i64, i64), Visit>, (x, y): (i64, i64)) -> u32 {
    let mut runs = 1;
    for dx in -1..=1 {
        for dy in -1..=1 {
            if let Some(visit) = visits.get(&(x + dx, y + dy)) {
                runs = runs.max(visit.runs);
            }
        }
    }
    runs
}

fn cell_of([x, y]: Position, cell: f64) -> (i64, i64) {
    ((x / cell).floor() as i64, (y / cell).floor() as i64)
}

fn midpoint(a: Position, b: Position) -> Position {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn distance(a: Position, b: Position) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Cuts a segment to the viewport with the Liang-Barsky algorithm.
fn clip(a: Position, b: Position, viewport: &Viewport) -> Option<[Position; 2]> {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let (mut enter, mut leave) = (0.0_f64, 1.0_f64);
    for axis in 0..2 {
        if delta[axis] == 0.0 {
            if a[axis] < viewport.min[axis] || a[axis] > viewport.max[axis] {
                return None;
            }
            continue;
        }
        let first = (viewport.min[axis] - a[axis]) / delta[axis];
        let second = (viewport.max[axis] - a[axis]) / delta[axis];
        enter = enter.max(first.min(second));
        leave = leave.min(first.max(second));
        if enter > leave {
            return None;
        }
    }
    let at = |t: f64| [a[0] + delta[0] * t, a[1] + delta[1] * t];
    // Exact end points, so that consecutive segments still join into one line.
    Some([
        if enter == 0.0 { a } else { at(enter) },
        if leave == 1.0 { b } else { at(leave) },
    ])
}

/// Ramer-Douglas-Peucker, with a stack because tracks can be very long.
fn simplify(path: &[Position], tolerance: f64) -> Vec<Position> {
    let mut keep = vec![false; path.len()];
    keep[0] = true;
    keep[path.len() - 1] = true;
    let mut stack = vec![(0, path.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        let farthest = (start + 1..end)
            .map(|index| {
                (
                    index,
                    distance_to_segment(path[index], path[start], path[end]),
                )
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((index, gap)) = farthest
            && gap > tolerance
        {
            keep[index] = true;
            stack.push((start, index));
            stack.push((index, end));
        }
    }
    path.iter()
        .zip(keep)
        .filter_map(|(position, kept)| kept.then_some(*position))
        .collect()
}

fn distance_to_segment(point: Position, a: Position, b: Position) -> f64 {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
    if length_squared == 0.0 {
        return distance(point, a);
    }
    let t = ((point[0] - a[0]) * delta[0] + (point[1] - a[1]) * delta[1]) / length_squared;
    let t = t.clamp(0.0, 1.0);
    distance(point, [a[0] + delta[0] * t, a[1] + delta[1] * t])
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: Viewport = Viewport {
        min: [0.0, 0.0],
        max: [1.0, 1.0],
    };
    /// One meter, roughly, in Mercator units at the equator.
    const METER: f64 = 2.5e-8;

    fn street(y: f64, length: usize) -> Track {
        (0..length)
            .map(|index| [0.5 + index as f64 * 3.0 * METER, y])
            .collect()
    }

    fn weights(lines: &[HeatLine]) -> Vec<f64> {
        lines.iter().map(|line| line.weight).collect()
    }

    #[test]
    fn projection_round_trips() {
        let [lon, lat] = unproject(project(52.52, 13.4));
        assert!((lon - 13.4).abs() < 1e-9);
        assert!((lat - 52.52).abs() < 1e-9);
    }

    #[test]
    fn viewport_has_north_at_the_smallest_y() {
        let viewport = Viewport::from_degrees(0.0, 40.0, 30.0, 60.0);
        assert!(viewport.min[0] < viewport.max[0]);
        assert!(viewport.min[1] < viewport.max[1]);
        let berlin = project(52.52, 13.4);
        assert!(berlin[0] > viewport.min[0] && berlin[0] < viewport.max[0]);
        assert!(berlin[1] > viewport.min[1] && berlin[1] < viewport.max[1]);
    }

    #[test]
    fn gaps_of_any_length_stay_connected() {
        let tunnel = vec![[0.5, 0.5], [0.6, 0.5]];
        let lines = heat_lines(&[tunnel], &WORLD, 18);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].path, [[0.5, 0.5], [0.6, 0.5]]);
    }

    #[test]
    fn segment_crossing_the_viewport_is_cut_to_it() {
        let viewport = Viewport {
            min: [0.55, 0.4],
            max: [0.56, 0.6],
        };
        let lines = heat_lines(&[vec![[0.5, 0.5], [0.6, 0.5]]], &viewport, 14);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].path, [[0.55, 0.5], [0.56, 0.5]]);
    }

    #[test]
    fn tracks_outside_the_viewport_are_left_out() {
        let viewport = Viewport {
            min: [0.1, 0.1],
            max: [0.2, 0.2],
        };
        assert!(heat_lines(&[vec![[0.5, 0.5], [0.6, 0.5]]], &viewport, 10).is_empty());
    }

    #[test]
    fn track_leaving_and_reentering_becomes_two_lines() {
        let viewport = Viewport {
            min: [0.0, 0.0],
            max: [0.3, 1.0],
        };
        let track = vec![
            [0.1, 0.5],
            [0.2, 0.5],
            [0.5, 0.5],
            [0.6, 0.5],
            [0.2, 0.6],
            [0.1, 0.6],
        ];
        let lines = heat_lines(&[track], &viewport, 4);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn repeated_routes_are_weighted_higher_than_quiet_ones() {
        let busy = street(0.5, 200);
        let quiet = street(0.6, 200);
        let tracks = vec![busy.clone(), busy.clone(), busy, quiet];
        let lines = heat_lines(&tracks, &WORLD, 14);
        assert!(weights(&lines).contains(&1.0));
        assert!(lines.iter().any(|line| line.weight < 1.0));
        // Sorted, so that busy lines are drawn on top.
        assert!(weights(&lines).is_sorted_by(|a, b| a <= b));
    }

    #[test]
    fn a_run_on_a_spot_many_times_counts_once() {
        let once = street(0.5, 200);
        let mut back_and_forth = once.clone();
        back_and_forth.extend(once.iter().rev());
        back_and_forth.extend(once.iter());
        let tracks = vec![back_and_forth];
        let lines = heat_lines(&tracks, &WORLD, 14);
        assert!(lines.iter().all(|line| line.weight == 1.0));
        let runs = count_runs(&tracks, &WORLD, MIN_COUNT_CELL);
        assert!(runs.values().all(|visit| visit.runs == 1));
    }

    #[test]
    fn straight_lines_are_simplified_and_curves_kept() {
        let lines = heat_lines(&[street(0.5, 200)], &WORLD, 14);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].path.len(), 2);

        let corner = vec![[0.5, 0.5], [0.5001, 0.5], [0.5001, 0.5001]];
        let lines = heat_lines(std::slice::from_ref(&corner), &WORLD, 14);
        assert_eq!(lines[0].path, corner);
    }

    #[test]
    fn repeated_points_do_not_break_anything() {
        let lines = heat_lines(&[vec![[0.5, 0.5], [0.5, 0.5], [0.5, 0.5]]], &WORLD, 18);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn scatter_between_cells_does_not_split_a_shared_route() {
        // Each run zigzags by a few meters around the same street.
        let run = |offset: f64| -> Track {
            (0..400)
                .map(|index| {
                    let side = if (index / 7) % 2 == 0 {
                        offset
                    } else {
                        -offset
                    };
                    [0.5 + index as f64 * 3.0 * METER, 0.5 + side * METER]
                })
                .collect()
        };
        let tracks = vec![run(2.0), run(5.0), run(9.0), run(13.0), run(1.0), run(7.0)];
        let lines = heat_lines(&tracks, &WORLD, 16);
        // One line per run, not many short pieces per run.
        assert!(lines.len() <= 2 * tracks.len(), "{} lines", lines.len());
    }

    #[test]
    fn no_tracks_mean_no_lines() {
        assert!(heat_lines(&[], &WORLD, 3).is_empty());
        assert!(heat_lines(&[vec![[0.5, 0.5]]], &WORLD, 3).is_empty());
    }
}
