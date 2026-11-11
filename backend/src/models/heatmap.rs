use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::heatmap::Position;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HeatmapQuery {
    /// Map zoom level from 0 to 22. Sets how much detail the lines keep.
    pub zoom: u8,
    /// Western edge of the viewport, from -180 to 180 degrees.
    pub west: f64,
    /// Southern edge of the viewport, from -90 to 90 degrees.
    pub south: f64,
    /// Eastern edge of the viewport, from -180 to 180 degrees.
    pub east: f64,
    /// Northern edge of the viewport, from -90 to 90 degrees.
    pub north: f64,
}

#[derive(Serialize, ToSchema)]
pub struct HeatLine {
    /// From 0 for the fewest activities in the viewport to 1 for the most, on a log scale.
    pub weight: f64,
    /// Longitude and latitude pairs.
    #[schema(value_type = Vec<Vec<f64>>)]
    pub path: Vec<Position>,
}

#[derive(Serialize, ToSchema)]
pub struct Heatmap {
    /// Sorted from the quietest line to the busiest.
    pub lines: Vec<HeatLine>,
}
