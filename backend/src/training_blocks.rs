//! Training blocks: the weeks that lead up to each race.

use anyhow::Result;

use crate::database::{self, Pool};
use crate::models::{RaceDay, TrainingBlock};

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
    let races = database::race_days(pool, user_id).await?;
    let mut blocks = training_blocks(&races);
    blocks.reverse();
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

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
