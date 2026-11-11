use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ActivityTypeQuery {
    /// Also counts activities without GPS, as the statistics do.
    #[serde(default)]
    pub without_gps: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityType {
    /// Garmin's key for the type, like `trail_running`.
    pub activity_type: String,
    pub activities: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ActivityTypeList {
    /// Most frequent first.
    pub types: Vec<ActivityType>,
}
