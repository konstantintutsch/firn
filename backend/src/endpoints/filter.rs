use crate::database::Pool;
use crate::models::{Filter, FilterQuery};
use crate::training_blocks;

impl FilterQuery {
    pub async fn into_filter(self, pool: &Pool, user_id: i64) -> anyhow::Result<Filter> {
        let query = self;
        let block = match query.block {
            Some(id) => training_blocks::training_blocks_of(pool, user_id)
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
pub(crate) fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
