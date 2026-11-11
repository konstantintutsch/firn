use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::database::schema::routes;

/// A point of a route: latitude, longitude and elevation in meters, if known.
pub type RoutePoint = (f64, f64, Option<f64>);

/// A saved route without its points.
#[derive(Debug, Queryable, Selectable, serde::Serialize, utoipa::ToSchema)]
#[diesel(table_name = routes)]
pub struct RouteSummary {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    /// The activity that the route was made from, if it still exists.
    pub activity_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// About 64 points along the route, as `[latitude, longitude]`, to draw a small picture of it.
    #[schema(value_type = Vec<Vec<f64>>)]
    pub preview: Value,
}

#[derive(Insertable)]
#[diesel(table_name = routes)]
pub struct NewRoute<'a> {
    pub user_id: i64,
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    pub points: &'a Value,
    pub preview: &'a Value,
    pub activity_id: Option<Uuid>,
}

/// A route as an export holds it.
#[derive(Debug, Queryable, Selectable, serde::Serialize, serde::Deserialize)]
#[diesel(table_name = routes)]
pub struct RouteRecord {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub distance_m: f64,
    pub elevation_gain_m: Option<f64>,
    pub points: Value,
    pub activity_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct RouteList {
    pub routes: Vec<RouteSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct RouteDetail {
    #[serde(flatten)]
    pub route: RouteSummary,
    /// `[latitude, longitude, elevation]` in degrees and meters, with `null` for
    /// an unknown elevation.
    #[schema(value_type = Vec<Vec<Option<f64>>>)]
    pub points: Vec<RoutePoint>,
}

#[derive(Deserialize, ToSchema)]
pub struct NewRouteFromActivity {
    /// The ID of the activity to take the track from.
    pub activity_id: Uuid,
    /// Defaults to the activity's title.
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct RouteChange {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RouteImportQuery {
    /// Defaults to `Imported route`.
    pub name: Option<String>,
}
