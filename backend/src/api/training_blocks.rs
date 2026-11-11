use axum::Json;
use axum::extract::State;

use super::{ApiError, AppState, ErrorBody};
use crate::auth::CurrentUser;
use crate::models::TrainingBlocks;
use crate::training_blocks;

/// The training blocks of the races that have an official result. A block runs
/// from the last race that was at least as long to the race itself, so shorter
/// races in between do not end it. Pass its `id` as `block` to filter by it.
#[utoipa::path(
    get,
    path = "/api/training-blocks",
    responses(
        (status = 200, body = TrainingBlocks),
        (status = 401, description = "Not signed in", body = ErrorBody),
    )
)]
pub(super) async fn training_blocks(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Result<Json<TrainingBlocks>, ApiError> {
    Ok(Json(TrainingBlocks {
        blocks: training_blocks::training_blocks_of(&state.pool, user.id).await?,
    }))
}
