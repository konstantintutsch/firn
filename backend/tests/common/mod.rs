#![allow(dead_code)]

use chrono::{Duration as Days, Utc};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use runmap::api::{AppState, GarminSettings};
use runmap::auth::{Auth, OidcSettings, SESSION_COOKIE};
use runmap::db::{self, Pool};
use runmap::files::FileStore;
use runmap::garmin::GarminClient;
use runmap::secret::{self, SecretBox};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wiremock::MockServer;
use zip::write::SimpleFileOptions;

static NEXT_DATABASE: AtomicU32 = AtomicU32::new(0);

/// Creates a fresh, migrated database so that tests stay independent.
pub async fn test_pool() -> Pool {
    let admin_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    // Not the process ID, which can repeat between containers: a failed `just test`
    // leaves its databases behind in a running test-db.
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let name = format!(
        "test_{started}_{}",
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    );
    let mut admin = AsyncPgConnection::establish(&admin_url).await.unwrap();
    diesel::sql_query(format!("CREATE DATABASE {name}"))
        .execute(&mut admin)
        .await
        .unwrap();
    let (base, _) = admin_url.rsplit_once('/').unwrap();
    let pool = db::connect(&format!("{base}/{name}")).unwrap();
    db::migrate(&pool).await.unwrap();
    pool
}

/// A file store of its own for each test database, named after it.
pub async fn files(pool: &Pool) -> FileStore {
    #[derive(diesel::QueryableByName)]
    struct Name {
        #[diesel(sql_type = diesel::sql_types::Text)]
        name: String,
    }
    let Name { name } = diesel::sql_query("SELECT current_database() AS name")
        .get_result(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    FileStore::new(std::env::temp_dir().join("runmap-tests").join(name))
}

pub const API_KEY: &str = "test-key";
pub const SECRET: &str = "a secret that is long enough for tests";
pub const ISSUER: &str = "https://issuer.example";

pub fn secret_box() -> SecretBox {
    SecretBox::new(SECRET).unwrap()
}

/// The account that tests act as, created on first use.
pub async fn user(pool: &Pool) -> i64 {
    user_named(pool, "tester").await
}

pub async fn user_named(pool: &Pool, subject: &str) -> i64 {
    let (user, _) = db::sign_in_user(pool, ISSUER, subject, None, Some(subject))
        .await
        .unwrap();
    user.id
}

/// A `Cookie` header with a new session of the account.
pub async fn cookie_for(pool: &Pool, user_id: i64) -> String {
    let token = secret::random_token();
    db::create_session(
        pool,
        user_id,
        &secret::sha256(&token),
        Utc::now() + Days::days(1),
    )
    .await
    .unwrap();
    format!("{SESSION_COOKIE}={token}")
}

pub async fn cookie(pool: &Pool) -> String {
    cookie_for(pool, user(pool).await).await
}

/// Saves `api_key` as the Garmin key of the test account.
pub async fn state(pool: Pool, base_url: &str, api_key: Option<&str>) -> AppState {
    state_with_oidc(pool, base_url, api_key, None).await
}

pub async fn state_with_oidc(
    pool: Pool,
    base_url: &str,
    api_key: Option<&str>,
    oidc: Option<OidcSettings>,
) -> AppState {
    let user_id = user(&pool).await;
    let sealed = api_key.map(|key| secret_box().seal(key).unwrap());
    db::set_garmin_key(&pool, user_id, sealed.as_deref())
        .await
        .unwrap();
    let files = files(&pool).await;
    AppState::new(
        pool,
        files,
        GarminSettings {
            base_url: base_url.to_owned(),
            api_key: None,
            download_pause: Duration::ZERO,
            retry_delay: Duration::ZERO,
            health_history_days: 2,
        },
        Auth::new(oidc, secret_box()),
    )
}

pub fn client(server: &MockServer) -> GarminClient {
    GarminClient::new(&server.uri(), API_KEY)
        .unwrap()
        .with_retry_delay(Duration::ZERO)
}

pub fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in files {
        archive
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(content).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

/// Encodes a minimal FIT file with one record message per position, in semicircles.
pub fn fit_of(positions: &[(i32, i32)]) -> Vec<u8> {
    const SINT32: u8 = 0x85;
    const RECORD_MESSAGE: u16 = 20;

    let mut data = vec![0x40, 0, 0];
    data.extend(RECORD_MESSAGE.to_le_bytes());
    data.extend([2, 0, 4, SINT32, 1, 4, SINT32]);
    for (lat, lon) in positions {
        data.push(0);
        data.extend(lat.to_le_bytes());
        data.extend(lon.to_le_bytes());
    }
    fit_file(data)
}

/// A FIT record with the metrics a watch records along an activity, in FIT's units.
pub struct FitRecord {
    pub lat: i32,
    pub lon: i32,
    /// Seconds since 1989-12-31.
    pub timestamp: u32,
    /// Meters times 5, plus 500.
    pub altitude: u16,
    pub heart_rate: u8,
    /// Centimeters.
    pub distance: u32,
    /// Millimeters per second.
    pub speed: u16,
}

/// Encodes a minimal FIT file with one record message per record.
pub fn fit_of_records(records: &[FitRecord]) -> Vec<u8> {
    const UINT8: u8 = 0x02;
    const UINT16: u8 = 0x84;
    const UINT32: u8 = 0x86;
    const SINT32: u8 = 0x85;
    const RECORD_MESSAGE: u16 = 20;

    let mut data = vec![0x40, 0, 0];
    data.extend(RECORD_MESSAGE.to_le_bytes());
    data.extend([7, 253, 4, UINT32, 0, 4, SINT32, 1, 4, SINT32, 2, 2, UINT16]);
    data.extend([3, 1, UINT8, 5, 4, UINT32, 6, 2, UINT16]);
    for record in records {
        data.push(0);
        data.extend(record.timestamp.to_le_bytes());
        data.extend(record.lat.to_le_bytes());
        data.extend(record.lon.to_le_bytes());
        data.extend(record.altitude.to_le_bytes());
        data.push(record.heart_rate);
        data.extend(record.distance.to_le_bytes());
        data.extend(record.speed.to_le_bytes());
    }
    fit_file(data)
}

/// Encodes a FIT file with one max_met_data message per `(category, vo2_max)`, where
/// the category is 0 for running and 1 for cycling, and VO2 max is in tenths.
pub fn fit_of_vo2_max(estimates: &[(u8, u16)]) -> Vec<u8> {
    const ENUM: u8 = 0x00;
    const UINT16: u8 = 0x84;
    const MAX_MET_DATA_MESSAGE: u16 = 229;

    let mut data = vec![0x40, 0, 0];
    data.extend(MAX_MET_DATA_MESSAGE.to_le_bytes());
    data.extend([2, 2, 2, UINT16, 8, 1, ENUM]);
    for (category, vo2_max) in estimates {
        data.push(0);
        data.extend(vo2_max.to_le_bytes());
        data.push(*category);
    }
    fit_file(data)
}

fn fit_file(data: Vec<u8>) -> Vec<u8> {
    let mut file = vec![14, 0x20];
    file.extend(2132u16.to_le_bytes());
    file.extend((data.len() as u32).to_le_bytes());
    file.extend(b".FIT");
    file.extend(fit_crc(&file).to_le_bytes());
    file.extend(data);
    file.extend(fit_crc(&file).to_le_bytes());
    file
}

fn fit_crc(bytes: &[u8]) -> u16 {
    const TABLE: [u16; 16] = [
        0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401, 0xA001, 0x6C00, 0x7800,
        0xB401, 0x5000, 0x9C01, 0x8801, 0x4400,
    ];
    bytes.iter().fold(0, |crc, byte| {
        let crc = (crc >> 4) ^ TABLE[usize::from(crc & 0xF)] ^ TABLE[usize::from(byte & 0xF)];
        (crc >> 4) ^ TABLE[usize::from(crc & 0xF)] ^ TABLE[usize::from((byte >> 4) & 0xF)]
    })
}

/// Semicircles for a coordinate in degrees.
pub fn semicircles(degrees: f64) -> i32 {
    (degrees * 2_147_483_648.0 / 180.0).round() as i32
}
