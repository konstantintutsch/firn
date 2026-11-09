use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use super::Pool;
use crate::schema::{activities, race_results};

/// A training block: the weeks from one race to the next that is at least as long.
/// Shorter races in between do not end it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct TrainingBlock {
    /// The activity of the race that ends the block.
    pub id: Uuid,
    /// The name of that race.
    pub name: String,
    /// The local day of that race, which is the block's last day.
    pub to: NaiveDate,
    /// The day after the race that began the block. `None` for the first race of
    /// its distance, whose block starts at the beginning.
    pub from: Option<NaiveDate>,
    /// How many races the block holds, its last one included.
    pub races: usize,
}

/// One race, as the training blocks need it.
pub struct RaceDay {
    pub id: Uuid,
    pub name: String,
    pub day: NaiveDate,
    pub distance_m: f64,
}

/// The blocks that end in each of the races, oldest race first.
pub fn training_blocks(races: &[RaceDay]) -> Vec<TrainingBlock> {
    races
        .iter()
        .enumerate()
        .map(|(index, race)| {
            // Within a meter counts as the same distance.
            let start = races[..index]
                .iter()
                .rposition(|earlier| earlier.distance_m >= race.distance_m - 1.0);
            TrainingBlock {
                id: race.id,
                name: race.name.clone(),
                to: race.day,
                from: start.and_then(|start| races[start].day.succ_opt()),
                races: index - start.map_or(0, |start| start + 1) + 1,
            }
        })
        .collect()
}

/// The account's training blocks, newest first.
pub async fn training_blocks_of(pool: &Pool, user_id: i64) -> Result<Vec<TrainingBlock>> {
    #[derive(Queryable)]
    struct Row {
        id: Uuid,
        name: String,
        event: Option<String>,
        started_at: DateTime<Utc>,
        distance_m: f64,
        started_local: Option<String>,
    }
    let rows: Vec<Row> = race_results::table
        .inner_join(activities::table)
        .filter(activities::user_id.eq(user_id))
        .order((activities::started_at, activities::id))
        .select((
            activities::id,
            activities::name,
            race_results::event,
            activities::started_at,
            race_results::distance_m,
            activities::started_local,
        ))
        .load(&mut pool.get().await?)
        .await?;
    let races: Vec<RaceDay> = rows
        .into_iter()
        .map(|row| RaceDay {
            id: row.id,
            name: row.event.unwrap_or(row.name),
            day: row
                .started_local
                .and_then(|local| local.get(..10)?.parse().ok())
                .unwrap_or_else(|| row.started_at.date_naive()),
            distance_m: row.distance_m,
        })
        .collect();
    let mut blocks = training_blocks(&races);
    blocks.reverse();
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_run_from_the_last_race_that_is_as_long() {
        let race = |day: &str, distance_m: f64| RaceDay {
            id: Uuid::nil(),
            name: String::new(),
            day: day.parse().unwrap(),
            distance_m,
        };
        let blocks = training_blocks(&[
            race("2025-03-01", 10_000.0),
            race("2025-04-01", 42_195.0),
            race("2025-06-01", 5_000.0),
            race("2025-07-01", 21_097.5),
            race("2025-10-01", 42_195.0),
        ]);
        let found: Vec<_> = blocks
            .iter()
            .map(|block| (block.from.map(|day| day.to_string()), block.races))
            .collect();
        assert_eq!(
            found,
            [
                (None, 1),
                (None, 2),
                (Some("2025-04-02".to_owned()), 1),
                (Some("2025-04-02".to_owned()), 2),
                (Some("2025-04-02".to_owned()), 3),
            ]
        );
    }
}
