use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::schema::users;

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = users)]
pub struct User {
    pub id: i64,
    pub email: Option<String>,
    pub name: Option<String>,
    pub garmin_api_key: Option<Vec<u8>>,
}

#[derive(Serialize, ToSchema)]
pub struct Me {
    pub id: i64,
    pub name: Option<String>,
    pub email: Option<String>,
    /// Whether a Garmin Personal API key is saved. The key itself is never returned.
    pub has_garmin_key: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct GarminKey {
    /// Created at https://connect.garmin.com/app/settings/personalApiToken
    pub api_key: String,
}
