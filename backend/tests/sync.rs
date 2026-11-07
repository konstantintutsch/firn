mod common;

use std::time::Duration;

use chrono::NaiveDate;
use common::{API_KEY, fit_of, semicircles, zip_of};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use runmap::db::{self, Pool};
use runmap::garmin::GarminError;
use runmap::schema::{activities, garmin_documents, track_points};
use runmap::sync::{HealthRange, Report, sync_activities, sync_health};
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
        "startLatitude": has_polyline.then_some(52.5),
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
    let user_id = common::user(pool).await;
    sync_activities(
        pool,
        &mut common::client(server),
        user_id,
        Duration::ZERO,
        |_| {},
    )
    .await
}

async fn is_track_synced(pool: &Pool, id: i64) -> bool {
    let user_id = common::user(pool).await;
    !db::missing(pool, user_id, id).await.unwrap().unwrap().track
}

async fn point_count(pool: &Pool, id: i64) -> i64 {
    track_points::table
        .filter(track_points::activity_id.eq(id))
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
            to_download: 3,
            downloaded: 3,
            failed: 0,
            ..Default::default()
        }
    );
    assert_eq!(point_count(&pool, 2).await, 2);

    let (name, original, summary): (String, Option<Vec<u8>>, Value) = activities::table
        .find(2)
        .select((
            activities::name,
            activities::original_zip,
            activities::summary,
        ))
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(name, "Run 2");
    assert_eq!(original, Some(original_zip()));
    assert_eq!(summary["startLatitude"], 52.5);
}

#[tokio::test]
async fn activity_of_another_account_is_left_unchanged() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    run_sync(&pool, &server).await.unwrap();

    let other = common::user_named(&pool, "other").await;
    let renamed = runmap::garmin::Activity {
        id: 1,
        name: "Renamed".to_owned(),
        activity_type: "cycling".to_owned(),
        started_at: chrono::Utc::now(),
        distance_m: None,
        duration_s: None,
        has_track: true,
        summary: json!({}),
    };
    db::upsert_activities(&pool, other, &[renamed])
        .await
        .unwrap();

    let (name, activity_type, owner): (String, String, Option<i64>) = activities::table
        .find(1)
        .select((
            activities::name,
            activities::activity_type,
            activities::user_id,
        ))
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(name, "Run 1");
    assert_eq!(activity_type, "running");
    assert_eq!(owner, Some(common::user(&pool).await));
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
async fn activities_without_gps_are_saved_but_left_off_the_map() {
    let (pool, server) = setup().await;
    mount_list(
        &server,
        &[vec![
            activity(1, "treadmill_running", false),
            activity(2, "virtual_run", true),
            activity(3, "strength_training", false),
            activity(4, "cycling", true),
        ]],
    )
    .await;
    for id in 1..=4 {
        mount_file(&server, id, 1).await;
    }

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!(
        report,
        Report {
            listed: 4,
            to_download: 4,
            downloaded: 4,
            failed: 0,
            ..Default::default()
        }
    );
    let filter = db::Filter {
        user_id: common::user(&pool).await,
        ..Default::default()
    };
    let totals = db::totals(&pool, &filter).await.unwrap();
    assert_eq!(totals.activities, 1);
    // The virtual run's file has a track, but in a virtual world.
    assert_eq!(point_count(&pool, 2).await, 0);
    assert!(is_track_synced(&pool, 2).await);
    assert!(is_track_synced(&pool, 4).await);
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
async fn server_errors_and_short_rate_limits_are_retried() {
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
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(file_path(2)))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    mount_file(&server, 1, 1).await;
    mount_file(&server, 2, 1).await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!((report.downloaded, report.failed), (2, 0));
    server.verify().await;
}

#[tokio::test]
async fn long_rate_limit_stops_without_waiting() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    Mock::given(method("GET"))
        .and(path(file_path(1)))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3600"))
        .expect(1)
        .mount(&server)
        .await;

    let result = run_sync(&pool, &server).await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
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
            to_download: 2,
            downloaded: 1,
            failed: 1,
            ..Default::default()
        }
    );
    assert!(!is_track_synced(&pool, 1).await);
    assert!(is_track_synced(&pool, 2).await);
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
            to_download: 2,
            downloaded: 1,
            failed: 1,
            ..Default::default()
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
    client.list_activities(0, 1).await.unwrap();
    client.list_activities(0, 1).await.unwrap();
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
    client.list_activities(0, 1).await.unwrap();
    client.list_activities(0, 1).await.unwrap();
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

#[tokio::test]
async fn saves_activity_details() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"summaryDTO": {"averageHR": 150}})),
        )
        .expect(1)
        .mount(&server)
        .await;

    run_sync(&pool, &server).await.unwrap();
    // Nothing is left to download, so a second sync asks for nothing.
    assert_eq!(run_sync(&pool, &server).await.unwrap().to_download, 0);
    let details: Option<Value> = activities::table
        .find(1)
        .select(activities::details)
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(details.unwrap()["summaryDTO"]["averageHR"], 150);
}

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

async fn run_health(pool: &Pool, server: &MockServer, today: &str) -> Report {
    let user_id = common::user(pool).await;
    let mut report = Report::default();
    sync_health(
        pool,
        &mut common::client(server),
        user_id,
        HealthRange {
            today: day(today),
            history_days: 10,
        },
        Duration::ZERO,
        &mut report,
        |_| {},
    )
    .await
    .unwrap();
    report
}

async fn health_requests(server: &MockServer, path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| request.url.path() == path)
        .count()
}

#[tokio::test]
async fn saves_health_data_and_reads_daily_values() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .and(path("/sleep-service/sleep/dailySleep"))
        .and(query_param("date", "2026-03-09"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "dailySleepDTO": {
                "calendarDate": "2026-03-09",
                "sleepTimeSeconds": 28800,
                "sleepScores": {"overall": {"value": 88}},
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex("^/hrv-service/hrv/daily/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "hrvSummaries": [{"calendarDate": "2026-03-08", "lastNightAvg": 52}]
        })))
        .mount(&server)
        .await;

    let report = run_health(&pool, &server, "2026-03-10").await;
    assert_eq!(report.health_failed, 0);
    assert_eq!(report.health_fetched, report.health_to_fetch);
    // Eleven days, of which none is in the future.
    assert_eq!(
        health_requests(&server, "/sleep-service/sleep/dailySleep").await,
        11
    );

    let user_id = common::user(&pool).await;
    let metrics = db::daily_metrics(&pool, user_id, None, None).await.unwrap();
    assert!(metrics.contains(&("sleep_h".to_owned(), day("2026-03-09"), 8.0)));
    assert!(metrics.contains(&("sleep_score".to_owned(), day("2026-03-09"), 88.0)));
    assert!(metrics.contains(&("hrv".to_owned(), day("2026-03-08"), 52.0)));

    let saved: Option<Value> = garmin_documents::table
        .filter(garmin_documents::kind.eq("sleep"))
        .filter(garmin_documents::period.eq("2026-03-09"))
        .select(garmin_documents::data)
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(saved.unwrap()["dailySleepDTO"]["sleepTimeSeconds"], 28800);
}

#[tokio::test]
async fn next_health_sync_fetches_only_recent_days_again() {
    let (pool, server) = setup().await;
    run_health(&pool, &server, "2026-03-10").await;
    server.reset().await;
    mount_token(&server).await;

    run_health(&pool, &server, "2026-03-11").await;
    // The new day and the days that Garmin may still change.
    assert_eq!(
        health_requests(&server, "/sleep-service/sleep/dailySleep").await,
        4
    );
}

#[tokio::test]
async fn data_that_garmin_refuses_is_skipped_for_the_rest_of_the_sync() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex("^/periodichealth-service/"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;

    let report = run_health(&pool, &server, "2026-03-10").await;
    assert_eq!(report.health_failed, 0);
    assert_eq!(report.health_fetched, report.health_to_fetch);
}

#[tokio::test]
async fn rate_limit_stops_the_health_sync() {
    let (pool, server) = setup().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let user_id = common::user(&pool).await;
    let result = sync_health(
        &pool,
        &mut common::client(&server),
        user_id,
        HealthRange {
            today: day("2026-03-10"),
            history_days: 10,
        },
        Duration::ZERO,
        &mut Report::default(),
        |_| {},
    )
    .await;
    assert!(matches!(result, Err(GarminError::RateLimited)));
}

#[tokio::test]
async fn saves_activity_workouts() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/1/workouts"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"workoutName": "Intervals"}])),
        )
        .expect(1)
        .mount(&server)
        .await;

    run_sync(&pool, &server).await.unwrap();
    assert_eq!(run_sync(&pool, &server).await.unwrap().to_download, 0);
    let workouts: Option<Value> = activities::table
        .find(1)
        .select(activities::workouts)
        .first(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    assert_eq!(workouts.unwrap()[0]["workoutName"], "Intervals");
}

#[tokio::test]
async fn activity_without_workouts_is_not_asked_again() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/1/workouts"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    run_sync(&pool, &server).await.unwrap();
    run_sync(&pool, &server).await.unwrap();
}

#[tokio::test]
async fn saves_activity_images_without_sending_the_token() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    let image_url = format!("{}/activity_images/a-larg.jpg?signature=1", server.uri());
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadataDTO": {"activityImages": [{"imageId": "a", "url": image_url}]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/activity_images/a-larg.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"jpeg".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    run_sync(&pool, &server).await.unwrap();
    // Nothing is left to download, so a second sync asks for nothing.
    assert_eq!(run_sync(&pool, &server).await.unwrap().to_download, 0);

    let requests = server.received_requests().await.unwrap();
    let image_request = requests
        .iter()
        .find(|request| request.url.path() == "/activity_images/a-larg.jpg")
        .unwrap();
    assert!(!image_request.headers.contains_key("authorization"));
    let user_id = common::user(&pool).await;
    assert_eq!(db::image_ids(&pool, 1).await.unwrap(), ["a"]);
    assert_eq!(
        db::image(&pool, user_id, 1, "a").await.unwrap(),
        Some(("image/jpeg".to_owned(), b"jpeg".to_vec()))
    );
}

#[tokio::test]
async fn saved_details_without_images_need_no_request() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    run_sync(&pool, &server).await.unwrap();
    // Activities saved before images were synced have details but no images yet.
    diesel::update(activities::table.find(1))
        .set(activities::images_synced_at.eq(None::<chrono::DateTime<chrono::Utc>>))
        .execute(&mut pool.get().await.unwrap())
        .await
        .unwrap();

    assert_eq!(run_sync(&pool, &server).await.unwrap().to_download, 0);
}

#[tokio::test]
async fn image_that_garmin_refuses_is_skipped_for_good() {
    let (pool, server) = setup().await;
    mount_list(&server, &[vec![activity(1, "running", true)]]).await;
    mount_file(&server, 1, 1).await;
    let image_url = format!("{}/activity_images/old-larg.jpg", server.uri());
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadataDTO": {"activityImages": [{"imageId": "old", "url": image_url}]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/activity_images/old-larg.jpg"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;

    let report = run_sync(&pool, &server).await.unwrap();
    assert_eq!((report.downloaded, report.failed), (1, 0));
    assert_eq!(run_sync(&pool, &server).await.unwrap().to_download, 0);
    assert!(db::image_ids(&pool, 1).await.unwrap().is_empty());
}
