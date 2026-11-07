mod common;

use std::time::Duration;

use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use runmap::db::{self, Pool};
use runmap::garmin::{GarminClient, GarminError};
use runmap::schema::{runs, track_points};
use runmap::sync::{Report, sync_runs};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST: &str = "/activitylist-service/activities/search/activities";
const GPX: &str = r#"<gpx><trk><trkseg>
    <trkpt lat="52.5" lon="13.4"/><trkpt lat="52.6" lon="13.5"/>
</trkseg></trk></gpx>"#;

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

fn gpx_path(id: i64) -> String {
    format!("/download-service/export/gpx/activity/{id}")
}

async fn mount_gpx(server: &MockServer, id: i64, expected_calls: u64) {
    Mock::given(method("GET"))
        .and(path(gpx_path(id)))
        .respond_with(ResponseTemplate::new(200).set_body_string(GPX))
        .expect(expected_calls)
        .mount(server)
        .await;
}

async fn run_sync(pool: &Pool, server: &MockServer) -> Result<Report, GarminError> {
    let mut client = GarminClient::from_saved_tokens(pool.clone(), common::endpoints(server))
        .await
        .unwrap();
    sync_runs(pool, &mut client, Duration::ZERO).await
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
    let pool = common::test_pool().await;
    db::save_tokens(&pool, &common::tokens()).await.unwrap();
    (pool, MockServer::start().await)
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
        mount_gpx(&server, id, 1).await;
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

    let (name, gpx, summary): (String, Option<String>, Value) = runs::table
        .find(2)
        .select((runs::name, runs::gpx, runs::summary))
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(name, "Run 2");
    assert_eq!(gpx.as_deref(), Some(GPX));
    assert_eq!(summary["hasPolyline"], true);
}

#[tokio::test]
async fn second_sync_downloads_only_new_runs() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_gpx(&server, 1, 1).await;
    run_sync(&pool, &server).await.unwrap();

    server.reset().await;
    mount_list(
        &server,
        &[vec![
            activity(2, "running", true),
            activity(1, "running", true),
        ]],
    )
    .await;
    mount_gpx(&server, 1, 0).await;
    mount_gpx(&server, 2, 1).await;

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
    mount_gpx(&server, 1, 0).await;
    mount_gpx(&server, 2, 0).await;

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
    mount_list(
        &server,
        &[vec![
            activity(1, "running", true),
            activity(2, "running", true),
            activity(3, "running", true),
        ]],
    )
    .await;
    mount_gpx(&server, 1, 1).await;
    Mock::given(method("GET"))
        .and(path(gpx_path(2)))
        .respond_with(ResponseTemplate::new(429))
        .expect(1)
        .mount(&server)
        .await;
    mount_gpx(&server, 3, 0).await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
    server.verify().await;

    server.reset().await;
    mount_list(
        &server,
        &[vec![
            activity(1, "running", true),
            activity(2, "running", true),
            activity(3, "running", true),
        ]],
    )
    .await;
    mount_gpx(&server, 1, 0).await;
    mount_gpx(&server, 2, 1).await;
    mount_gpx(&server, 3, 1).await;
    assert_eq!(run_sync(&pool, &server).await.unwrap().downloaded, 2);
}

#[tokio::test]
async fn broken_track_is_skipped_and_retried_later() {
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
        .and(path(gpx_path(1)))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<gpx><trkpt lat=\"x\" lon=\"0\"/></gpx>"),
        )
        .mount(&server)
        .await;
    mount_gpx(&server, 2, 1).await;

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
async fn expired_token_is_refreshed_and_saved() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(header("authorization", "Bearer old-access"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("refresh_token=old-refresh"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"access_token": "new-access", "refresh_token": "new-refresh"}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(header("authorization", "Bearer new-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    run_sync(&pool, &server).await.unwrap();
    let saved = db::load_tokens(&pool).await.unwrap().unwrap();
    assert_eq!(saved.access_token, "new-access");
    assert_eq!(saved.refresh_token, "new-refresh");
}

#[tokio::test]
async fn refused_refresh_asks_for_login() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::NeedsLogin)));
}

#[tokio::test]
async fn sync_without_login_asks_for_login() {
    let pool = common::test_pool().await;
    let server = MockServer::start().await;
    let result = GarminClient::from_saved_tokens(pool, common::endpoints(&server)).await;
    assert!(matches!(result, Err(GarminError::NeedsLogin)));
}
