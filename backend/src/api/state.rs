use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::AbortHandle;

use crate::auth::Auth;
use crate::database::{self, Pool};
use crate::files::FileStore;
use crate::models::{ActivityTrack, SyncStatus};

/// Accounts whose tracks stay in memory. More would be rare on one server.
const MAX_CACHED_ACCOUNTS: usize = 32;

struct CachedTracks {
    tracks: Arc<tokio::sync::OnceCell<Arc<Vec<ActivityTrack>>>>,
    used: Instant,
}

pub struct GarminSettings {
    pub base_url: String,
    pub download_pause: Duration,
    /// The wait before retrying a failed Garmin request.
    pub retry_delay: Duration,
    /// How far back health data goes at least.
    pub health_history_days: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: Pool,
    pub(crate) garmin: Arc<GarminSettings>,
    pub(crate) auth: Arc<Auth>,
    pub(crate) files: FileStore,
    /// By account.
    pub(super) sync: Arc<Mutex<HashMap<i64, SyncStatus>>>,
    /// The running syncs by account, to cancel them.
    pub(super) sync_tasks: Arc<Mutex<HashMap<i64, AbortHandle>>>,
    /// By account. Loaded on the first request and again after a sync, because the
    /// heatmap walks every track on every request, which would be slow from the database.
    tracks: Arc<Mutex<HashMap<i64, CachedTracks>>>,
}

impl AppState {
    pub fn new(pool: Pool, files: FileStore, garmin: GarminSettings, auth: Auth) -> Self {
        Self {
            pool,
            files,
            garmin: Arc::new(garmin),
            auth: Arc::new(auth),
            sync: Arc::default(),
            sync_tasks: Arc::default(),
            tracks: Arc::default(),
        }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Stops the account's running sync. False if none runs.
    pub(crate) fn cancel_sync(&self, user_id: i64) -> bool {
        match self.sync_tasks.lock().unwrap().get(&user_id) {
            Some(task) => {
                task.abort();
                true
            }
            None => false,
        }
    }

    /// Loads each account's tracks once. Loading for one account never waits for another.
    pub(super) async fn tracks(&self, user_id: i64) -> anyhow::Result<Arc<Vec<ActivityTrack>>> {
        let cell = {
            let mut cache = self.tracks.lock().unwrap();
            if !cache.contains_key(&user_id) && cache.len() >= MAX_CACHED_ACCOUNTS {
                // The account that went longest without a request makes room.
                if let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.used)
                    .map(|(id, _)| *id)
                {
                    cache.remove(&oldest);
                }
            }
            let cached = cache.entry(user_id).or_insert_with(|| CachedTracks {
                tracks: Arc::default(),
                used: Instant::now(),
            });
            cached.used = Instant::now();
            cached.tracks.clone()
        };
        let tracks = cell
            .get_or_try_init(|| async {
                database::load_tracks(&self.pool, user_id)
                    .await
                    .map(Arc::new)
            })
            .await?;
        Ok(tracks.clone())
    }

    /// Makes the next request load the account's tracks again, after a sync changed them.
    pub(crate) fn forget_tracks(&self, user_id: i64) {
        self.tracks.lock().unwrap().remove(&user_id);
    }
}
