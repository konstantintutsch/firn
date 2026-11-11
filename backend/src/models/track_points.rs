use uuid::Uuid;

use crate::heatmap::{Track, Viewport};

pub struct ActivityTrack {
    pub id: Uuid,
    pub track: Track,
    /// Around the whole track, so that the heatmap can skip tracks outside the viewport.
    pub bounds: Viewport,
}
