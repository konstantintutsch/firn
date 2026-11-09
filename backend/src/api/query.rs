use chrono::NaiveDate;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::db::{self, Filter, Pool};

/// Limits which activities count. Empty parameters are ignored and all others must match.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FilterQuery {
    /// One activity.
    id: Option<Uuid>,
    /// Words to find in the title, type, description, location or local start time.
    /// A length like `10km` or `45min` also finds activities within 5% of it.
    q: Option<String>,
    /// Comma-separated Garmin activity types, like `running,cycling`.
    types: Option<String>,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    min_distance_m: Option<f64>,
    max_distance_m: Option<f64>,
    min_duration_s: Option<f64>,
    max_duration_s: Option<f64>,
    /// First day to include, in local time.
    from: Option<NaiveDate>,
    /// Last day to include, in local time.
    to: Option<NaiveDate>,
    /// A training block, by the ID of the race that ends it. Leaves out the
    /// activities from before and after it. Unknown blocks are ignored.
    block: Option<Uuid>,
}

impl FilterQuery {
    pub async fn into_filter(self, pool: &Pool, user_id: i64) -> anyhow::Result<Filter> {
        let query = self;
        let block = match query.block {
            Some(id) => db::training_blocks_of(pool, user_id)
                .await?
                .into_iter()
                .find(|block| block.id == id),
            None => None,
        };
        // The block narrows the days that were asked for.
        let from = match (query.from, block.as_ref().and_then(|block| block.from)) {
            (Some(from), Some(start)) => Some(from.max(start)),
            (from, start) => from.or(start),
        };
        let to = match (query.to, block.as_ref().map(|block| block.to)) {
            (Some(to), Some(end)) => Some(to.min(end)),
            (to, end) => to.or(end),
        };
        Ok(Filter {
            user_id,
            id: query.id,
            query: text(query.q),
            activity_types: query
                .types
                .iter()
                .flat_map(|types| types.split(','))
                .map(str::trim)
                .filter(|activity_type| !activity_type.is_empty())
                .map(str::to_owned)
                .collect(),
            title: text(query.title),
            description: text(query.description),
            location: text(query.location),
            min_distance_m: query.min_distance_m,
            max_distance_m: query.max_distance_m,
            min_duration_s: query.min_duration_s,
            max_duration_s: query.max_duration_s,
            from,
            to,
            without_gps: false,
        })
    }
}

/// Trimmed text, or `None` if nothing is left.
pub(super) fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
