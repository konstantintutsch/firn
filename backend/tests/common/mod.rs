#![allow(dead_code)]

use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use runmap::db::{self, Pool};
use runmap::garmin::GarminClient;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use wiremock::MockServer;
use zip::write::SimpleFileOptions;

static NEXT_DATABASE: AtomicU32 = AtomicU32::new(0);

/// Creates a fresh, migrated database so that tests stay independent.
pub async fn test_pool() -> Pool {
    let admin_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL is required");
    let name = format!(
        "test_{}_{}",
        std::process::id(),
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

pub const API_KEY: &str = "test-key";

pub fn client(server: &MockServer) -> GarminClient {
    GarminClient::new(&server.uri(), API_KEY).unwrap()
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
