//! Turns tracks into colored lines for a map viewport.
//!
//! Positions are Web Mercator coordinates in the unit square.

mod projection;

pub use projection::{Position, Track, Viewport, bounds, project, unproject};

use std::collections::HashMap;

/// World width in pixels at zoom 0 for MapLibre's 512 px tiles.
const WORLD_PIXELS: f64 = 512.0;
/// Runs count as passing the same spot if they share a cell this wide.
const COUNT_CELL_PIXELS: f64 = 4.0;
/// About 12 m at the equator. Runs on one street spread over this much GPS error,
/// so smaller cells would split them into seemingly different routes.
const MIN_COUNT_CELL: f64 = 3e-7;
/// Simplifies lines by up to this much, which is too little to see.
const SIMPLIFY_PIXELS: f64 = 0.25;
/// Number of color steps. Fewer steps merge more segments into one line, but make
/// the jump between two neighboring colors easier to see.
const LEVELS: u32 = 16;

#[derive(Debug, PartialEq)]
pub struct HeatLine {
    /// From 0 for the fewest runs in the viewport to 1 for the most, on a log scale.
    pub weight: f64,
    pub path: Vec<Position>,
}

/// Builds the lines to draw, sorted so that the busiest are last and end up on top.
///
/// Consecutive points always connect, however far apart: a gap usually means lost
/// GPS, as in a tunnel, and the straight line shows that the run went through.
pub fn heat_lines<T: AsRef<[Position]>>(
    tracks: &[T],
    viewport: &Viewport,
    zoom: u8,
) -> Vec<HeatLine> {
    let pixel = 1.0 / (WORLD_PIXELS * 2f64.powi(zoom.into()));
    let cell = (COUNT_CELL_PIXELS * pixel).max(MIN_COUNT_CELL);
    let nearby = nearby_runs(&runs_per_cell(tracks, viewport, cell), tracks.len());
    let max_runs = nearby.values().copied().max().unwrap_or(1);
    let level_of = |[a, b]: [Position; 2]| {
        // The midpoint represents a long gap best, but counting walks in steps, so it can
        // land in a corner of a cell that was skipped. The start is always counted.
        let runs = nearby
            .get(&cell_of(midpoint(a, b), cell))
            .or_else(|| nearby.get(&cell_of(a, cell)))
            .copied()
            .unwrap_or(1);
        let share = f64::from(runs).ln_1p() / f64::from(max_runs).ln_1p();
        (share * f64::from(LEVELS - 1)).round() as u32
    };

    let mut pieces: Vec<(u32, Vec<Position>)> = Vec::new();
    for track in tracks {
        let mut current: Option<(u32, Vec<Position>)> = None;
        for segment in track.as_ref().windows(2) {
            let Some([a, b]) = clip(segment[0], segment[1], viewport) else {
                pieces.extend(current.take());
                continue;
            };
            let level = level_of([a, b]);
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

type Cell = (i64, i64);

/// The tracks that pass through each cell, without repeats.
fn runs_per_cell<T: AsRef<[Position]>>(
    tracks: &[T],
    viewport: &Viewport,
    cell: f64,
) -> HashMap<Cell, Vec<u32>> {
    let mut runs: HashMap<Cell, Vec<u32>> = HashMap::new();
    for (index, track) in tracks.iter().enumerate() {
        let index = index as u32;
        for segment in track.as_ref().windows(2) {
            let Some([a, b]) = clip(segment[0], segment[1], viewport) else {
                continue;
            };
            let steps = (distance(a, b) / cell).ceil().max(1.0);
            for step in 0..=steps as u64 {
                let t = step as f64 / steps;
                let position = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                let runs = runs.entry(cell_of(position, cell)).or_default();
                // Tracks are walked one at a time, so a repeat is always the last entry.
                if runs.last() != Some(&index) {
                    runs.push(index);
                }
            }
        }
    }
    runs
}

/// Counts the distinct runs in each cell and its neighbors.
///
/// GPS error scatters a shared route over several cells, and how many runs a single
/// cell catches depends on how the grid happens to line up with the street. Counting
/// the whole neighborhood sees all of them, so the color stays the same along the way.
fn nearby_runs(runs: &HashMap<Cell, Vec<u32>>, track_count: usize) -> HashMap<Cell, u32> {
    // Marks the tracks seen for the current cell, without clearing it for every cell.
    let mut seen_at = vec![usize::MAX; track_count];
    runs.keys()
        .enumerate()
        .map(|(round, &(x, y))| {
            let mut count = 0;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &track in runs.get(&(x + dx, y + dy)).into_iter().flatten() {
                        if seen_at[track as usize] != round {
                            seen_at[track as usize] = round;
                            count += 1;
                        }
                    }
                }
            }
            ((x, y), count)
        })
        .collect()
}

fn cell_of([x, y]: Position, cell: f64) -> Cell {
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
        let runs = runs_per_cell(&tracks, &WORLD, MIN_COUNT_CELL);
        assert!(runs.values().all(|runs| runs.len() == 1));
    }

    #[test]
    fn viewports_intersect_when_they_overlap_or_touch() {
        let viewport = |min: Position, max: Position| Viewport { min, max };
        let a = viewport([0.0, 0.0], [1.0, 1.0]);
        assert!(a.intersects(&viewport([0.5, 0.5], [2.0, 2.0])));
        assert!(a.intersects(&viewport([1.0, 0.0], [2.0, 1.0])));
        assert!(a.intersects(&viewport([0.2, 0.2], [0.3, 0.3])));
        assert!(!a.intersects(&viewport([1.1, 0.0], [2.0, 1.0])));
        assert!(!a.intersects(&viewport([0.0, 1.1], [1.0, 2.0])));
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

    /// Runs on one street, each off by up to `spread` meters and wandering slowly.
    fn scattered_street(runs: usize, spread: f64, angle: f64) -> Vec<Track> {
        (0..runs)
            .map(|run| {
                let offset = spread * (2.0 * run as f64 / (runs - 1) as f64 - 1.0);
                (0..1000)
                    .map(|index| {
                        let along = index as f64 * 3.0 * METER;
                        let wander = (offset + (index as f64 / 40.0 + run as f64).sin()) * METER;
                        [
                            0.5 + along * angle.cos() - wander * angle.sin(),
                            0.5 + along * angle.sin() + wander * angle.cos(),
                        ]
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_shared_street_keeps_one_color_however_the_grid_lines_up() {
        for angle in [0.0, 0.3, 0.8, 1.2] {
            for zoom in [14, 16, 18] {
                let mut tracks = scattered_street(40, 8.0, angle);
                // Sets the top of the scale, far enough away not to affect the street.
                tracks.extend(vec![street(0.7, 50); 200]);
                let lines = heat_lines(&tracks, &WORLD, zoom);
                let on_street: Vec<f64> = lines
                    .iter()
                    .filter(|line| line.path[0][1] < 0.65)
                    .map(|line| line.weight)
                    .collect();
                let (min, max) = on_street.iter().fold((1.0_f64, 0.0_f64), |(min, max), w| {
                    (min.min(*w), max.max(*w))
                });
                // The ends of the street taper off, so allow a single color step.
                assert!(
                    max - min <= 1.0 / f64::from(LEVELS - 1) + 1e-9,
                    "angle {angle}, zoom {zoom}: weights from {min} to {max}"
                );
            }
        }
    }

    #[test]
    fn neighboring_cells_count_each_run_once() {
        let runs = HashMap::from([
            ((0, 0), vec![0, 1]),
            ((0, 1), vec![1, 2]),
            ((5, 5), vec![3]),
        ]);
        let nearby = nearby_runs(&runs, 4);
        assert_eq!(nearby[&(0, 0)], 3);
        assert_eq!(nearby[&(0, 1)], 3);
        assert_eq!(nearby[&(5, 5)], 1);
    }

    #[test]
    fn bounds_surround_the_whole_track() {
        let bounds = bounds(&[[0.5, 0.2], [0.1, 0.4], [0.3, 0.3]]).unwrap();
        assert_eq!(bounds.min, [0.1, 0.2]);
        assert_eq!(bounds.max, [0.5, 0.4]);
        assert!(super::bounds(&[]).is_none());
    }

    #[test]
    fn no_tracks_mean_no_lines() {
        assert!(heat_lines::<Track>(&[], &WORLD, 3).is_empty());
        assert!(heat_lines(&[vec![[0.5, 0.5]]], &WORLD, 3).is_empty());
    }
}
