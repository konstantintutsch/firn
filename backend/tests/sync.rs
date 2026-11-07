mod common;

use std::time::Duration;

use common::{API_KEY, fit_of, semicircles, zip_of};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use runmap::db::{self, Pool};
use runmap::garmin::GarminError;
use runmap::schema::{runs, track_points};
use runmap::sync::{Report, sync_runs};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST: &str = "/activitylist-service/activities/search/activities";
const TOKEN: &str = "/personalapitoken-service/personal-api/personal-api-key/token";

fn original_zip() -> Vec<u8> {
    let fit = fit_of(&[
        (semicircles(52.5), semicircles(13.4)),
        (semicircles(52.6), semicircles(13.5)),
    ]);
    zip_of(&[("activity.fit", &fit)])
}

fn activity(id: i64, type_key: &str, has_polyline: bool) -> Value {
    json!({
        "activityId": id,
        "activityName": format!("Run {id}"),
        "startTimeGMT": "2026-03-01 08:00:00",
        "activityType": {"typeKey": type_key},
        "distance": 5000.0,
        "duration": 1500.0,
        "hasPolyline": has_polyline,
    })
}

async fn mount_token(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .and(header("authorization", format!("ApiKey {API_KEY}")))
        .respond_with(ResponseTemplate::new(200).set_body_string("access\n"))
        .mount(server)
        .await;
}

async fn mount_list(server: &MockServer, pages: &[Vec<Value>]) {
    let mut start = 0;
    for page in pages {
        Mock::given(method("GET"))
            .and(path(LIST))
            .and(query_param("start", start.to_string()))
            .and(query_param("activityType", "running"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page))
            .mount(server)
            .await;
        start += page.len();
    }
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("start", start.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
}

fn file_path(id: i64) -> String {
    format!("/download-service/files/activity/{id}")
}

async fn mount_file(server: &MockServer, id: i64, expected_calls: u64) {
    Mock::given(method("GET"))
        .and(path(file_path(id)))
        .and(header("authorization", "Bearer access"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(original_zip()))
        .expect(expected_calls)
        .mount(server)
        .await;
}

async fn run_sync(pool: &Pool, server: &MockServer) -> Result<Report, GarminError> {
    sync_runs(pool, &mut common::client(server), Duration::ZERO).await
}

async fn point_count(pool: &Pool, id: i64) -> i64 {
    track_points::table
        .filter(track_points::run_id.eq(id))
        .count()
        .get_result(&mut pool.get().await.unwrap())
        .await
        .unwrap()
}

async fn setup() -> (Pool, MockServer) {
    let server = MockServer::start().await;
    mount_token(&server).await;
    (common::test_pool().await, server)
}

#[tokio::test]
async fn saves_all_pages_and_keeps_raw_data() {
    let (pool, server) = setup().await;
    mount_list(
        &server,
        &[
            vec![
                activity(1, "running", true),
                activity(2, "trail_running", true),
            ],
            vec![activity(3, "running", true)],
        ],
    )
    .await;
    for id in 1..=3 {
        mount_file(&server, id, 1).await;
    }

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(
        report,
        Report {
            listed: 3,
            downloaded: 3,
            failed: 0
        }
    );
    assert_eq!(point_count(&pool, 2).await, 2);

    let (name, original, summary): (String, Option<Vec<u8>>, Value) = runs::table
        .find(2)
        .select((runs::name, runs::original_zip, runs::summary))
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(name, "Run 2");
    assert_eq!(original, Some(original_zip()));
    assert_eq!(summary["hasPolyline"], true);
}

#[tokio::test]
async fn second_sync_downloads_only_new_runs() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    run_sync(&pool, &server).await.unwrap();

    server.reset().await;
    mount_token(&server).await;
    mount_list(
        &server,
        &[vec![
            activity(2, "running", true),
            activity(1, "running", true),
        ]],
    )
    .await;
    mount_file(&server, 1, 0).await;
    mount_file(&server, 2, 1).await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(report.downloaded, 1);
    assert_eq!(point_count(&pool, 1).await, 2);
}

#[tokio::test]
async fn runs_without_gps_are_saved_without_a_download() {
    let (pool, server) = setup().await;
    mount_list(
        &server,
        &[vec![
            activity(1, "treadmill_running", false),
            activity(2, "virtual_run", true),
        ]],
    )
    .await;
    mount_file(&server, 1, 0).await;
    mount_file(&server, 2, 0).await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(
        report,
        Report {
            listed: 2,
            downloaded: 0,
            failed: 0
        }
    );
    assert_eq!(db::totals(&pool).await.unwrap().runs, 2);
    assert!(db::is_track_synced(&pool, 1).await.unwrap());
}

#[tokio::test]
async fn rate_limit_stops_the_sync_and_next_sync_resumes() {
    let (pool, server) = setup().await;
    let runs = vec![
        activity(1, "running", true),
        activity(2, "running", true),
        activity(3, "running", true),
    ];
    mount_list(&server, std::slice::from_ref(&runs)).await;
    mount_file(&server, 1, 1).await;
    Mock::given(method("GET"))
        .and(path(file_path(2)))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;
    mount_file(&server, 3, 0).await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
    server.verify().await;

    server.reset().await;
    mount_token(&server).await;
    mount_list(&server, std::slice::from_ref(&runs)).await;
    mount_file(&server, 1, 0).await;
    mount_file(&server, 2, 1).await;
    mount_file(&server, 3, 1).await;
    assert_eq!(run_sync(&pool, &server).await.unwrap().downloaded, 2);
}

#[tokio::test]
async fn broken_file_is_skipped_and_retried_later() {
    let (pool, server) = setup().await;
    mount_list(
        &server,
        &[vec![
            activity(1, "running", true),
            activity(2, "running", true),
        ]],
    )
    .await;
    Mock::given(method("GET"))
        .and(path(file_path(1)))
        .respond_with(ResponseTemplate::new(200).set_body_string("not a zip"))
        .mount(&server)
        .await;
    mount_file(&server, 2, 1).await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(
        report,
        Report {
            listed: 2,
            downloaded: 1,
            failed: 1
        }
    );
    assert!(!db::is_track_synced(&pool, 1).await.unwrap());
    assert!(db::is_track_synced(&pool, 2).await.unwrap());
}

#[tokio::test]
async fn forbidden_file_is_skipped_and_others_still_download() {
    let (pool, server) = setup().await;
    mount_list(
        &server,
        &[vec![
            activity(1, "running", true),
            activity(2, "running", true),
        ]],
    )
    .await;
    Mock::given(method("GET"))
        .and(path(file_path(1)))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    mount_file(&server, 2, 1).await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(
        report,
        Report {
            listed: 2,
            downloaded: 1,
            failed: 1
        }
    );
}

#[tokio::test]
async fn rate_limited_list_stops_the_sync() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
}

#[tokio::test]
async fn revoked_token_is_replaced_once() {
    let server = MockServer::start().await;
    let pool = common::test_pool().await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string("first"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string("second"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(header("authorization", "Bearer first"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(header("authorization", "Bearer second"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    assert_eq!(run_sync(&pool, &server).await.unwrap().listed, 0);
}

#[tokio::test]
async fn expiring_token_is_replaced_before_use() {
    let server = MockServer::start().await;
    // The payload is {"exp":1}: expired long ago.
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string("h.eyJleHAiOjF9.s"))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let mut client = common::client(&server);
    client.list_runs(0, 1).await.unwrap();
    client.list_runs(0, 1).await.unwrap();
}

#[tokio::test]
async fn valid_token_is_reused() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string("access"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let mut client = common::client(&server);
    client.list_runs(0, 1).await.unwrap();
    client.list_runs(0, 1).await.unwrap();
}

#[tokio::test]
async fn refused_api_key_is_reported_without_retry() {
    let server = MockServer::start().await;
    let pool = common::test_pool().await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::InvalidApiKey)));
}
