use chrono::NaiveDate;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

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

#[derive(Serialize, ToSchema)]
pub struct TrainingBlocks {
    /// Newest first.
    pub blocks: Vec<TrainingBlock>,
}
