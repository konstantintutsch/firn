use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct Stats {
    pub activities: i64,
    pub distance_m: f64,
    /// West, south, east and north edges around all matching tracks, in degrees.
    #[schema(value_type = Option<Vec<f64>>)]
    pub bounds: Option<[f64; 4]>,
}
