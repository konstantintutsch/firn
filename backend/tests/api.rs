mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use runmap::api::{AppState, router};
use runmap::db::{self, Pool};
use runmap::track::Point;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LIST: &str = "/activitylist-service/activities/search/activities";
const TOKEN: &str = "/personalapitoken-service/personal-api/personal-api-key/token";

async fn get(pool: &Pool, uri: &str) -> (StatusCode, Value) {
    let cookie = common::cookie(pool).await;
    get_with(pool, uri, Some(&cookie)).await
}

async fn get_with(pool: &Pool, uri: &str, cookie: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::get(uri);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = router(common::state(pool.clone(), "http://unused", None).await)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn add_run(pool: &Pool, id: i64, points: &[Point]) {
    let summary = serde_json::json!({
        "activityId": id,
        "startTimeGMT": "2026-03-01 08:00:00",
        "activityType": {"typeKey": "running"},
        "distance": 1000.0,
    });
    let activity = runmap::garmin::Activity {
        id,
        name: format!("Run {id}"),
        activity_type: "running".into(),
        started_at: chrono::Utc::now(),
        distance_m: Some(1000.0),
        duration_s: None,
        has_track: true,
        summary,
    };
    db::upsert_activities(pool, common::user(pool).await, &[activity])
        .await
        .unwrap();
    db::save_track(pool, id, points).await.unwrap();
}

const BERLIN: Point = Point {
    lat: 52.52,
    lon: 13.40,
};
const SYDNEY: Point = Point {
    lat: -33.87,
    lon: 151.21,
};
const WORLD: &str = "west=-180&south=-90&east=180&north=90";

fn line_points(body: &Value) -> Vec<(f64, f64)> {
    body["lines"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|line| line["path"].as_array().unwrap())
        .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
        .collect()
}

#[tokio::test]
async fn returns_lines_in_degrees_through_the_track() {
    let pool = common::test_pool().await;
    let east = Point {
        lat: BERLIN.lat + 0.001,
        lon: BERLIN.lon + 0.002,
    };
    add_run(&pool, 1, &[BERLIN, east]).await;

    let (status, body) = get(&pool, &format!("/api/heatmap?zoom=14&{WORLD}")).await;
    assert_eq!(status, StatusCode::OK);
    let points = line_points(&body);
    assert_eq!(points.len(), 2);
    assert!((points[0].0 - BERLIN.lon).abs() < 1e-6);
    assert!((points[0].1 - BERLIN.lat).abs() < 1e-6);
    assert!((points[1].0 - east.lon).abs() < 1e-6);
    assert!((points[1].1 - east.lat).abs() < 1e-6);
}

#[tokio::test]
async fn connects_points_across_a_long_gps_gap() {
    let pool = common::test_pool().await;
    add_run(&pool, 1, &[BERLIN, SYDNEY]).await;

    let (_, body) = get(&pool, &format!("/api/heatmap?zoom=10&{WORLD}")).await;
    assert_eq!(body["lines"].as_array().unwrap().len(), 1);
    assert_eq!(line_points(&body).len(), 2);
}

#[tokio::test]
async fn gap_shows_in_a_viewport_between_its_end_points() {
    let pool = common::test_pool().await;
    add_run(
        &pool,
        1,
        &[
            BERLIN,
            Point {
                lat: BERLIN.lat,
                lon: BERLIN.lon + 1.0,
            },
        ],
    )
    .await;

    let west = BERLIN.lon + 0.4;
    let east = BERLIN.lon + 0.6;
    let south = BERLIN.lat - 0.1;
    let north = BERLIN.lat + 0.1;
    let (_, body) = get(
        &pool,
        &format!("/api/heatmap?zoom=12&west={west}&south={south}&east={east}&north={north}"),
    )
    .await;
    let points = line_points(&body);
    assert_eq!(points.len(), 2);
    assert!((points[0].0 - west).abs() < 1e-6);
    assert!((points[1].0 - east).abs() < 1e-6);
}

#[tokio::test]
async fn viewport_excludes_other_tracks() {
    let pool = common::test_pool().await;
    add_run(
        &pool,
        1,
        &[
            BERLIN,
            Point {
                lat: 52.53,
                lon: 13.41,
            },
        ],
    )
    .await;
    add_run(
        &pool,
        2,
        &[
            SYDNEY,
            Point {
                lat: -33.86,
                lon: 151.22,
            },
        ],
    )
    .await;

    let (_, body) = get(
        &pool,
        "/api/heatmap?zoom=5&west=0&south=40&east=30&north=60",
    )
    .await;
    let points = line_points(&body);
    assert_eq!(points.len(), 2);
    assert!(points.iter().all(|(lon, _)| *lon < 30.0));
}

#[tokio::test]
async fn popular_routes_weigh_more_and_come_last() {
    let pool = common::test_pool().await;
    let popular = [
        BERLIN,
        Point {
            lat: BERLIN.lat,
            lon: BERLIN.lon + 0.002,
        },
    ];
    let quiet = [
        SYDNEY,
        Point {
            lat: SYDNEY.lat,
            lon: SYDNEY.lon + 0.002,
        },
    ];
    add_run(&pool, 1, &popular).await;
    add_run(&pool, 2, &popular).await;
    add_run(&pool, 3, &popular).await;
    add_run(&pool, 4, &quiet).await;

    let (_, body) = get(&pool, &format!("/api/heatmap?zoom=14&{WORLD}")).await;
    let lines = body["lines"].as_array().unwrap();
    assert!(lines[0]["weight"].as_f64().unwrap() < 1.0);
    assert_eq!(lines.last().unwrap()["weight"], 1.0);
    assert!(lines.last().unwrap()["path"][0][1].as_f64().unwrap() > 0.0);
    assert!(lines[0]["path"][0][1].as_f64().unwrap() < 0.0);
}

#[tokio::test]
async fn zoom_above_the_map_maximum_still_works() {
    let pool = common::test_pool().await;
    add_run(
        &pool,
        1,
        &[
            BERLIN,
            Point {
                lat: 52.5201,
                lon: 13.4001,
            },
        ],
    )
    .await;
    let (status, _) = get(&pool, &format!("/api/heatmap?zoom=22&{WORLD}")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn rejects_invalid_viewports() {
    let pool = common::test_pool().await;
    for query in [
        format!("zoom=23&{WORLD}"),
        "zoom=1&west=10&south=0&east=0&north=1".to_owned(),
        "zoom=1&west=0&south=5&east=1&north=1".to_owned(),
        "zoom=1&west=-181&south=0&east=0&north=1".to_owned(),
        "zoom=1".to_owned(),
    ] {
        let (status, _) = get(&pool, &format!("/api/heatmap?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
}

#[tokio::test]
async fn stats_sum_all_runs() {
    let pool = common::test_pool().await;
    let (_, empty) = get(&pool, "/api/stats").await;
    assert_eq!(empty["activities"], 0);
    assert_eq!(empty["distance_m"], 0.0);
    assert_eq!(empty["bounds"], Value::Null);

    add_run(&pool, 1, &[BERLIN]).await;
    add_run(&pool, 2, &[SYDNEY]).await;
    // Older syncs saved activities without GPS, which no longer count.
    add_run(&pool, 3, &[]).await;
    let (_, body) = get(&pool, "/api/stats").await;
    assert_eq!(body["activities"], 2);
    assert_eq!(body["distance_m"], 2000.0);
    let bounds: Vec<f64> = serde_json::from_value(body["bounds"].clone()).unwrap();
    let expected = [BERLIN.lon, SYDNEY.lat, SYDNEY.lon, BERLIN.lat];
    for (actual, expected) in bounds.iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-6, "{bounds:?}");
    }
    let (_, filtered) = get(&pool, "/api/stats?id=1").await;
    assert!((filtered["bounds"][0].as_f64().unwrap() - BERLIN.lon).abs() < 1e-6);
}

const NEAR_BERLIN: Point = Point {
    lat: 52.53,
    lon: 13.41,
};
const NEAR_SYDNEY: Point = Point {
    lat: -33.88,
    lon: 151.22,
};

async fn add_activity(pool: &Pool, id: i64, summary: Value, points: &[Point]) {
    let mut full = json!({
        "activityId": id,
        "activityName": "Untitled",
        "startTimeGMT": "2026-03-01 08:00:00",
        "startTimeLocal": "2026-03-01 09:00:00",
        "activityType": {"typeKey": "running"},
        "distance": 5000.0,
        "duration": 1800.0,
    });
    full.as_object_mut()
        .unwrap()
        .extend(summary.as_object().unwrap().clone());
    let activity = runmap::garmin::Activity {
        id,
        name: full["activityName"].as_str().unwrap().to_owned(),
        activity_type: full["activityType"]["typeKey"].as_str().unwrap().to_owned(),
        started_at: chrono::Utc::now(),
        distance_m: full["distance"].as_f64(),
        duration_s: full["duration"].as_f64(),
        has_track: true,
        summary: full,
    };
    db::upsert_activities(pool, common::user(pool).await, &[activity])
        .await
        .unwrap();
    db::save_track(pool, id, points).await.unwrap();
}

async fn add_two_activities(pool: &Pool) {
    add_activity(
        pool,
        1,
        json!({
            "activityName": "Morning 100% loop",
            "description": "Windy along the river",
            "locationName": "Berlin",
        }),
        &[BERLIN, NEAR_BERLIN],
    )
    .await;
    add_activity(
        pool,
        2,
        json!({
            "activityName": "Harbour ride",
            "activityType": {"typeKey": "road_biking"},
            "locationName": "Sydney",
            "startTimeLocal": "2026-06-15 23:30:00",
            "distance": 40000.0,
            "duration": 7200.0,
        }),
        &[SYDNEY, NEAR_SYDNEY],
    )
    .await;
}

async fn found_ids(pool: &Pool, query: &str) -> Vec<i64> {
    let (status, body) = get(pool, &format!("/api/activities?{query}")).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    let mut ids: Vec<i64> = body["activities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|activity| activity["id"].as_i64().unwrap())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn search_finds_activities_by_everything() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;

    for (query, expected) in [
        ("q=morning", vec![1]),
        ("q=WINDY", vec![1]),
        ("q=sydney", vec![2]),
        ("q=road_biking", vec![2]),
        ("q=2026-06", vec![2]),
        ("q=5km", vec![1]),
        ("q=40%20km", vec![2]),
        ("q=2h", vec![2]),
        ("q=berlin%205km", vec![1]),
        ("q=berlin%2040km", vec![]),
        ("q=100%25", vec![1]),
        ("q=_", vec![2]),
        ("q=nothing", vec![]),
        ("q=", vec![1, 2]),
        ("id=2", vec![2]),
        ("types=road_biking,cycling", vec![2]),
        ("types=running,road_biking", vec![1, 2]),
        ("title=loop", vec![1]),
        ("title=river", vec![]),
        ("description=river", vec![1]),
        ("location=berl", vec![1]),
        ("min_distance_m=10000", vec![2]),
        ("max_distance_m=10000", vec![1]),
        ("min_duration_s=3600", vec![2]),
        ("max_duration_s=3600", vec![1]),
        ("from=2026-03-02", vec![2]),
        ("to=2026-03-01", vec![1]),
        ("from=2026-06-15&to=2026-06-15", vec![2]),
        ("location=berlin&types=road_biking", vec![]),
    ] {
        assert_eq!(found_ids(&pool, query).await, expected, "{query}");
    }
}

#[tokio::test]
async fn search_returns_details_newest_first_up_to_the_limit() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;

    let (status, body) = get(&pool, "/api/activities?q=berlin").await;
    assert_eq!(status, StatusCode::OK);
    let activity = &body["activities"][0];
    assert_eq!(activity["name"], "Morning 100% loop");
    assert_eq!(activity["location"], "Berlin");
    assert_eq!(activity["description"], "Windy along the river");
    let bounds: Vec<f64> = activity["bounds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge.as_f64().unwrap())
        .collect();
    let expected = [BERLIN.lon, BERLIN.lat, NEAR_BERLIN.lon, NEAR_BERLIN.lat];
    for (edge, expected) in bounds.iter().zip(expected) {
        assert!((edge - expected).abs() < 1e-6, "{bounds:?}");
    }

    let (_, limited) = get(&pool, "/api/activities?limit=1").await;
    assert_eq!(limited["activities"].as_array().unwrap().len(), 1);
    let (_, next) = get(&pool, "/api/activities?limit=1&offset=1").await;
    assert_eq!(next["activities"].as_array().unwrap().len(), 1);
    assert_ne!(next["activities"][0]["id"], limited["activities"][0]["id"]);
    let (_, past_end) = get(&pool, "/api/activities?offset=2").await;
    assert!(past_end["activities"].as_array().unwrap().is_empty());
    let (status, _) = get(&pool, "/api/activities?offset=-1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&pool, "/api/activities?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&pool, "/api/activities?limit=101").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn filters_apply_to_heatmap_and_stats() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;

    let lines_at = async |filter: &str| {
        let uri = format!("/api/heatmap?zoom=14&{WORLD}&{filter}");
        let (status, body) = get(&pool, &uri).await;
        assert_eq!(status, StatusCode::OK);
        line_points(&body)
    };
    assert_eq!(lines_at("").await.len(), 4);
    let points = lines_at("types=road_biking").await;
    assert_eq!(points.len(), 2);
    assert!(points.iter().all(|(lon, _)| *lon > 100.0));
    assert_eq!(lines_at("id=1").await.len(), 2);
    assert!(lines_at("location=nowhere").await.is_empty());

    let (_, stats) = get(&pool, "/api/stats?types=road_biking").await;
    assert_eq!(stats["activities"], 1);
    assert_eq!(stats["distance_m"], 40000.0);
}

#[tokio::test]
async fn lists_activity_types_most_frequent_first() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    add_activity(&pool, 3, json!({}), &[BERLIN]).await;
    add_activity(&pool, 4, json!({"activityType": {"typeKey": "yoga"}}), &[]).await;

    let (status, body) = get(&pool, "/api/activity-types").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["types"],
        json!([
            {"activity_type": "running", "activities": 2},
            {"activity_type": "road_biking", "activities": 1},
        ])
    );

    let (_, all) = get(&pool, "/api/activity-types?without_gps=true").await;
    assert_eq!(all["types"].as_array().unwrap().len(), 3);
    assert_eq!(
        all["types"][2],
        json!({"activity_type": "yoga", "activities": 1})
    );
}

#[tokio::test]
async fn publishes_openapi_without_secrets() {
    let pool = common::test_pool().await;
    let (status, spec) = get(&pool, "/api/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(spec["paths"]["/api/heatmap"].is_object());
    assert!(spec["paths"]["/api/activities"].is_object());
    assert!(spec["paths"]["/api/activity-types"].is_object());
    assert!(spec["paths"]["/api/stats"].is_object());
}

async fn request_sync(state: &AppState, with_header: bool) -> (StatusCode, Value) {
    let mut request =
        Request::post("/api/sync").header("cookie", common::cookie(state.pool()).await);
    if with_header {
        request = request.header("x-requested-with", "runmap");
    }
    let response = router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn sync_status(state: &AppState) -> Value {
    let request = Request::get("/api/sync").header("cookie", common::cookie(state.pool()).await);
    let response = router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn finished_sync(state: &AppState) -> Value {
    for _ in 0..500 {
        let status = sync_status(state).await;
        if status["state"] != "running" {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the sync did not finish");
}

async fn mount_garmin(server: &MockServer, delay: Duration) {
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_string("access"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("start", "0"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(delay)
                .set_body_json(json!([{
                    "activityId": 1,
                    "activityName": "Run",
                    "startTimeGMT": "2026-03-01 08:00:00",
                    "activityType": {"typeKey": "running"},
                    "distance": 5000.0,
                    "hasPolyline": true,
                }])),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("start", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
    let fit = common::fit_of(&[
        (
            common::semicircles(BERLIN.lat),
            common::semicircles(BERLIN.lon),
        ),
        (
            common::semicircles(BERLIN.lat),
            common::semicircles(BERLIN.lon + 0.001),
        ),
    ]);
    Mock::given(method("GET"))
        .and(path("/download-service/files/activity/1"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::zip_of(&[("a.fit", &fit)])))
        .mount(server)
        .await;
}

#[tokio::test]
async fn sync_starts_in_the_background_and_saves_runs() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::ZERO).await;
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), &server.uri(), Some(common::API_KEY)).await;
    assert_eq!(sync_status(&state).await["state"], "idle");

    let (status, body) = request_sync(&state, true).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["state"], "running");

    let finished = finished_sync(&state).await;
    assert_eq!(finished["state"], "succeeded");
    assert_eq!(finished["listed"], 1);
    assert_eq!(finished["to_download"], 1);
    assert_eq!(finished["downloaded"], 1);
    assert_eq!(
        db::totals(
            &pool,
            &db::Filter {
                user_id: common::user(&pool).await,
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .activities,
        1
    );

    // Can run again, and has nothing left to download.
    request_sync(&state, true).await;
    let again = finished_sync(&state).await;
    assert_eq!(again["state"], "succeeded");
    assert_eq!(again["downloaded"], 0);
}

#[tokio::test]
async fn sync_requires_the_header_that_other_websites_cannot_send() {
    let server = MockServer::start().await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;

    let (status, _) = request_sync(&state, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(sync_status(&state).await["state"], "idle");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn only_one_sync_runs_at_a_time() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::from_millis(300)).await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;

    assert_eq!(request_sync(&state, true).await.0, StatusCode::ACCEPTED);
    assert_eq!(request_sync(&state, true).await.0, StatusCode::CONFLICT);
    assert_eq!(finished_sync(&state).await["state"], "succeeded");
}

#[tokio::test]
async fn sync_without_a_key_explains_what_to_do() {
    let pool = common::test_pool().await;
    let state = common::state(pool, "http://unused", None).await;

    let (status, body) = request_sync(&state, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("settings"));
    assert_eq!(sync_status(&state).await["state"], "idle");
}

#[tokio::test]
async fn failed_sync_reports_the_error_without_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(TOKEN))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;

    request_sync(&state, true).await;
    let finished = finished_sync(&state).await;
    assert_eq!(finished["state"], "failed");
    let error = finished["error"].as_str().unwrap();
    assert!(error.contains("refused the API key"));
    assert!(!finished.to_string().contains(common::API_KEY));
}

async fn cancel_sync(state: &AppState) -> StatusCode {
    let request = Request::delete("/api/sync")
        .header("cookie", common::cookie(state.pool()).await)
        .header("x-requested-with", "runmap");
    router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_running_sync_can_be_cancelled() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::from_secs(30)).await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;

    assert_eq!(cancel_sync(&state).await, StatusCode::CONFLICT);
    request_sync(&state, true).await;
    // Gives the sync a moment to start its request to Garmin.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(cancel_sync(&state).await, StatusCode::NO_CONTENT);
    let finished = finished_sync(&state).await;
    assert_eq!(finished["state"], "cancelled");
    assert!(finished["finished_at"].is_string());
}

#[tokio::test]
async fn last_sync_survives_a_restart() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::ZERO).await;
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), &server.uri(), Some(common::API_KEY)).await;
    request_sync(&state, true).await;
    assert_eq!(finished_sync(&state).await["state"], "succeeded");

    // A new state has nothing in memory, like after a restart.
    let restarted = common::state(pool, &server.uri(), Some(common::API_KEY)).await;
    for _ in 0..100 {
        let status = sync_status(&restarted).await;
        if status["state"] == "succeeded" {
            assert_eq!(status["downloaded"], 1);
            assert!(status["finished_at"].is_string());
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the last sync was not saved");
}

#[tokio::test]
async fn map_shows_new_runs_after_a_sync() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::ZERO).await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;
    let lines = async |state: &AppState| {
        let uri = format!("/api/heatmap?zoom=10&{WORLD}");
        let request = Request::get(uri).header("cookie", common::cookie(state.pool()).await);
        let response = router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        body["lines"].as_array().unwrap().len()
    };

    // Also fills the tracks in memory, which the sync has to replace.
    assert_eq!(lines(&state).await, 0);
    request_sync(&state, true).await;
    finished_sync(&state).await;
    assert_eq!(lines(&state).await, 1);
}

#[tokio::test]
async fn activity_has_its_summary_and_samples() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    let fit = common::fit_of_records(&[
        common::FitRecord {
            lat: common::semicircles(BERLIN.lat),
            lon: common::semicircles(BERLIN.lon),
            timestamp: 1_000_000_000,
            altitude: (34 + 500) * 5,
            heart_rate: 150,
            distance: 0,
            speed: 3_250,
        },
        common::FitRecord {
            lat: i32::MAX,
            lon: i32::MAX,
            timestamp: 1_000_000_010,
            altitude: (36 + 500) * 5,
            heart_rate: 152,
            distance: 3_250,
            speed: 3_000,
        },
    ]);
    let zip = common::zip_of(&[("1.fit", &fit)]);
    db::save_track(&pool, 1, &[BERLIN]).await.unwrap();
    let files = common::files(&pool).await;
    files.save_original(1, Some(&zip)).await.unwrap();
    files.save_original(2, Some(b"zip")).await.unwrap();

    let (status, body) = get(&pool, "/api/activities/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["activity"]["name"], "Morning 100% loop");
    assert_eq!(body["summary"]["description"], "Windy along the river");
    let samples = &body["samples"];
    assert_eq!(samples["time_s"], json!([0.0, 10.0]));
    assert_eq!(samples["heart_rate"], json!([150.0, 152.0]));
    assert_eq!(samples["elevation_m"], json!([34.0, 36.0]));
    assert_eq!(samples["distance_m"], json!([0.0, 32.5]));
    assert_eq!(samples["position"][1], Value::Null);
    assert!((samples["position"][0][1].as_f64().unwrap() - BERLIN.lat).abs() < 1e-6);
    assert!(samples.get("power_w").is_none());
    assert_eq!(body["original_format"], "fit");

    // The second activity's file is not a ZIP, so it has no samples.
    let (status, body) = get(&pool, "/api/activities/2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["samples"], json!({}));
    assert_eq!(body["activity"]["bounds"], Value::Null);

    let (status, _) = get(&pool, "/api/activities/3").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn accounts_see_only_their_own_activities() {
    let pool = common::test_pool().await;
    add_run(&pool, 1, &[BERLIN, NEAR_BERLIN]).await;
    let other = common::user_named(&pool, "other").await;
    let other_cookie = common::cookie_for(&pool, other).await;

    let (_, mine) = get(&pool, "/api/stats").await;
    assert_eq!(mine["activities"], 1);
    for uri in ["/api/stats", "/api/activities", "/api/activity-types"] {
        let (status, body) = get_with(&pool, uri, Some(&other_cookie)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.to_string().contains("Run 1"), "{uri}");
    }
    let (status, _) = get_with(&pool, "/api/activities/1", Some(&other_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, theirs) = get_with(&pool, "/api/stats", Some(&other_cookie)).await;
    assert_eq!(theirs["activities"], 0);
    let (_, lines) = get_with(
        &pool,
        &format!("/api/heatmap?zoom=14&{WORLD}"),
        Some(&other_cookie),
    )
    .await;
    assert!(lines["lines"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn activity_statistics_include_activities_without_gps() {
    let pool = common::test_pool().await;
    add_activity(
        &pool,
        1,
        json!({"averageHR": 151.0, "elevationGain": 40.0}),
        &[BERLIN, NEAR_BERLIN],
    )
    .await;
    db::save_vo2_max(&pool, 1, Some(52.4)).await.unwrap();
    add_activity(
        &pool,
        2,
        json!({
            "activityName": "Gym",
            "activityType": {"typeKey": "strength_training"},
            "startTimeLocal": "2026-04-02 18:00:00",
        }),
        &[],
    )
    .await;

    let (status, body) = get(&pool, "/api/statistics/activities").await;
    assert_eq!(status, StatusCode::OK);
    let activities = body["activities"].as_array().unwrap();
    assert_eq!(activities.len(), 2);
    let run = activities
        .iter()
        .find(|activity| activity["id"] == 1)
        .unwrap();
    assert_eq!(run["average_heart_rate"], 151.0);
    assert_eq!(run["vo2_max"], 52.4);
    assert_eq!(run["result"], Value::Null);
    let gym = activities
        .iter()
        .find(|activity| activity["id"] == 2)
        .unwrap();
    assert_eq!(gym["day"], "2026-04-02");

    let (_, april) = get(
        &pool,
        "/api/statistics/activities?from=2026-04-01&to=2026-04-30",
    )
    .await;
    assert_eq!(april["activities"].as_array().unwrap().len(), 1);
    let (_, strength) = get(&pool, "/api/statistics/activities?types=strength_training").await;
    assert_eq!(strength["activities"].as_array().unwrap().len(), 1);
    assert_eq!(strength["activities"][0]["id"], 2);
    let (_, gym) = get(&pool, "/api/statistics/activities?title=gym").await;
    assert_eq!(gym["activities"].as_array().unwrap().len(), 1);
    let (status, _) = get(
        &pool,
        "/api/statistics/activities?from=2026-05-01&to=2026-04-30",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn health_statistics_group_values_by_metric() {
    let pool = common::test_pool().await;
    let user_id = common::user(&pool).await;
    let day = |text: &str| text.parse::<chrono::NaiveDate>().unwrap();
    db::save_document(
        &pool,
        user_id,
        "hrv_summaries",
        "2026-01-01/2026-01-28",
        Some(&json!({})),
        &[
            (day("2026-01-02"), "hrv", 50.0),
            (day("2026-01-01"), "hrv", 48.0),
            (day("2026-01-01"), "resting_heart_rate", 47.0),
        ],
    )
    .await
    .unwrap();

    let (status, body) = get(&pool, "/api/statistics/health?from=2026-01-01").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["metrics"]["hrv"],
        json!([["2026-01-01", 48.0], ["2026-01-02", 50.0]])
    );
    assert_eq!(
        body["metrics"]["resting_heart_rate"],
        json!([["2026-01-01", 47.0]])
    );

    let (_, later) = get(&pool, "/api/statistics/health?from=2026-01-02").await;
    assert_eq!(later["metrics"]["hrv"], json!([["2026-01-02", 50.0]]));
}

#[tokio::test]
async fn health_statistics_average_the_resting_heart_rate_over_a_week() {
    let pool = common::test_pool().await;
    let user_id = common::user(&pool).await;
    let day = |text: &str| text.parse::<chrono::NaiveDate>().unwrap();
    db::save_document(
        &pool,
        user_id,
        "user_summaries",
        "2026-01-01/2026-01-28",
        Some(&json!({})),
        &[
            (day("2026-01-01"), "resting_heart_rate", 50.0),
            (day("2026-01-07"), "resting_heart_rate", 46.0),
            (day("2026-01-08"), "resting_heart_rate", 42.0),
        ],
    )
    .await
    .unwrap();

    // The days before the range still count toward its first averages.
    let (status, body) = get(&pool, "/api/statistics/health?from=2026-01-07").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["metrics"]["resting_heart_rate"],
        json!([["2026-01-07", 46.0], ["2026-01-08", 42.0]])
    );
    assert_eq!(
        body["metrics"]["resting_heart_rate_7d"],
        json!([["2026-01-07", 48.0], ["2026-01-08", 44.0]])
    );
}

#[tokio::test]
async fn downloads_the_original_file_and_gpx() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    let fit = common::fit_of(&[
        (
            common::semicircles(BERLIN.lat),
            common::semicircles(BERLIN.lon),
        ),
        (
            common::semicircles(NEAR_BERLIN.lat),
            common::semicircles(NEAR_BERLIN.lon),
        ),
    ]);
    common::files(&pool)
        .await
        .save_original(1, Some(&common::zip_of(&[("1_ACTIVITY.fit", &fit)])))
        .await
        .unwrap();
    let state = common::state(pool.clone(), "http://unused", None).await;
    let download = async |uri: &str| {
        let request = Request::get(uri).header("cookie", common::cookie(&pool).await);
        let response = router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let disposition = response
            .headers()
            .get("content-disposition")
            .map(|value| value.to_str().unwrap().to_owned());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, disposition, bytes)
    };

    let (status, disposition, bytes) = download("/api/activities/1/file").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disposition.unwrap(), r#"attachment; filename="1.fit""#);
    assert_eq!(bytes.as_ref(), fit.as_slice());

    let (status, disposition, bytes) = download("/api/activities/1/file?format=gpx").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disposition.unwrap(), r#"attachment; filename="1.gpx""#);
    let gpx = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(gpx.contains("<name>Morning 100% loop</name>"));
    assert_eq!(gpx.matches("<trkpt").count(), 2);

    assert_eq!(
        download("/api/activities/1/file?format=kml").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        download("/api/activities/2/file").await.0,
        StatusCode::NOT_FOUND
    );
    let other = common::user_named(&pool, "other").await;
    let request = Request::get("/api/activities/1/file")
        .header("cookie", common::cookie_for(&pool, other).await);
    let response = router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn one_activity_can_be_downloaded_again() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::ZERO).await;
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), &server.uri(), Some(common::API_KEY)).await;
    request_sync(&state, true).await;
    assert_eq!(finished_sync(&state).await["state"], "succeeded");
    let downloads = async || {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/download-service/files/activity/1")
            .count()
    };
    assert_eq!(downloads().await, 1);

    let resync = async |id: i64, cookie: String| {
        let request = Request::post(format!("/api/activities/{id}/sync"))
            .header("cookie", cookie)
            .header("x-requested-with", "runmap");
        router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    };
    assert_eq!(
        resync(1, common::cookie(&pool).await).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(downloads().await, 2);
    assert_eq!(
        resync(99, common::cookie(&pool).await).await,
        StatusCode::NOT_FOUND
    );
    let other = common::user_named(&pool, "other").await;
    assert_eq!(
        resync(1, common::cookie_for(&pool, other).await).await,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn activity_links_its_neighbors_and_has_its_zones() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    let zones = json!([
        {"sport": "DEFAULT", "zone1Floor": 100, "zone2Floor": 120, "zone3Floor": 140,
         "zone4Floor": 160, "zone5Floor": 175, "maxHeartRateUsed": 190},
        {"sport": "RUNNING", "zone1Floor": 101, "zone2Floor": 121, "zone3Floor": 141,
         "zone4Floor": 161, "zone5Floor": 176, "maxHeartRateUsed": 191},
    ]);
    db::save_document(
        &pool,
        common::user(&pool).await,
        "heart_rate_zones",
        "",
        Some(&zones),
        &[],
    )
    .await
    .unwrap();

    let (_, first) = get(&pool, "/api/activities/1").await;
    assert_eq!(first["previous_id"], Value::Null);
    assert_eq!(first["next_id"], 2);
    assert_eq!(
        first["heart_rate_zones"],
        json!([101.0, 121.0, 141.0, 161.0, 176.0, 191.0])
    );
    let (_, second) = get(&pool, "/api/activities/2").await;
    assert_eq!(second["previous_id"], 1);
    assert_eq!(second["next_id"], Value::Null);
    // A ride has no zones of its own here, so it gets the default ones.
    assert_eq!(second["heart_rate_zones"][0], 100.0);
}

#[tokio::test]
async fn search_sorts_and_can_include_activities_without_gps() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    add_activity(
        &pool,
        3,
        json!({"activityName": "Gym", "activityType": {"typeKey": "strength_training"}, "distance": null}),
        &[],
    )
    .await;

    assert_eq!(ordered_ids(&pool, "").await, [2, 1]);
    assert_eq!(ordered_ids(&pool, "sort=oldest").await, [1, 2]);
    assert_eq!(ordered_ids(&pool, "sort=distance").await, [2, 1]);
    assert_eq!(ordered_ids(&pool, "without_gps=true").await, [3, 2, 1]);
    assert_eq!(
        ordered_ids(&pool, "without_gps=true&sort=distance").await,
        [2, 1, 3]
    );
    let (status, _) = get(&pool, "/api/activities?sort=random").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn ordered_ids(pool: &Pool, query: &str) -> Vec<i64> {
    let (status, body) = get(pool, &format!("/api/activities?{query}")).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    body["activities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|activity| activity["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn deleting_the_account_removes_its_data_and_files() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    let files = common::files(&pool).await;
    files.save_original(1, Some(b"zip")).await.unwrap();
    let other = common::user_named(&pool, "other").await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    let cookie = common::cookie(&pool).await;

    let delete = |cookie: &str, header: bool| {
        let mut request = Request::delete("/api/me").header("cookie", cookie);
        if header {
            request = request.header("x-requested-with", "runmap");
        }
        router(state.clone()).oneshot(request.body(Body::empty()).unwrap())
    };
    assert_eq!(
        delete(&cookie, false).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let response = delete(&cookie, true).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );

    assert_eq!(
        get_with(&pool, "/api/me", Some(&cookie)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(db::owner(&pool, 1).await.unwrap(), None);
    assert_eq!(files.original(1).await.unwrap(), None);
    // Other accounts stay.
    let (status, _) = get_with(
        &pool,
        "/api/me",
        Some(&common::cookie_for(&pool, other).await),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn health_needs_no_session() {
    let pool = common::test_pool().await;
    let (status, _) = get_with(&pool, "/api/health", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn photos_are_only_for_their_account() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    db::save_images(
        &pool,
        1,
        &[db::Image {
            id: "a".to_owned(),
            content_type: "image/png".to_owned(),
        }],
    )
    .await
    .unwrap();
    common::files(&pool)
        .await
        .save_image(1, "a", b"png")
        .await
        .unwrap();
    let state = common::state(pool.clone(), "http://unused", None).await;
    let fetch = async |cookie: String| {
        let request = Request::get("/api/activities/1/images/a").header("cookie", cookie);
        let response = router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response.headers().get("content-type").cloned();
        (status, content_type)
    };

    let (status, content_type) = fetch(common::cookie(&pool).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.unwrap(), "image/png");
    let other = common::user_named(&pool, "other").await;
    let (status, _) = fetch(common::cookie_for(&pool, other).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn race_results_are_entered_by_hand() {
    let pool = common::test_pool().await;
    add_two_activities(&pool).await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    let send = async |method: &str, id: i64, body: Value, cookie: String| {
        let request = Request::builder()
            .method(method)
            .uri(format!("/api/activities/{id}/result"))
            .header("cookie", cookie)
            .header("x-requested-with", "runmap")
            .header("content-type", "application/json");
        router(state.clone())
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
            .status()
    };
    let result = json!({
        "distance_m": 21097.5,
        "time_s": 5400.0,
        "event": " Berlin Half ",
        "results_url": "https://results.example/123",
    });

    let cookie = common::cookie(&pool).await;
    assert_eq!(
        send("PUT", 1, result.clone(), cookie.clone()).await,
        StatusCode::NO_CONTENT
    );
    let (_, detail) = get(&pool, "/api/activities/1").await;
    assert_eq!(detail["result"]["event"], "Berlin Half");
    assert_eq!(detail["result"]["time_s"], 5400.0);
    let (_, statistics) = get(&pool, "/api/statistics/activities").await;
    let first = &statistics["activities"].as_array().unwrap()[0];
    assert_eq!(
        first["result"]["results_url"],
        "https://results.example/123"
    );

    let mut bad = result.clone();
    bad["results_url"] = json!("javascript:alert(1)");
    assert_eq!(
        send("PUT", 1, bad, cookie.clone()).await,
        StatusCode::BAD_REQUEST
    );
    let mut bad = result.clone();
    bad["time_s"] = json!(0);
    assert_eq!(
        send("PUT", 1, bad, cookie.clone()).await,
        StatusCode::BAD_REQUEST
    );
    let other = common::user_named(&pool, "other").await;
    let other_cookie = common::cookie_for(&pool, other).await;
    assert_eq!(
        send("PUT", 1, result.clone(), other_cookie.clone()).await,
        StatusCode::NOT_FOUND
    );
    // Another account cannot remove it either.
    send("DELETE", 1, Value::Null, other_cookie).await;
    assert_eq!(
        get(&pool, "/api/activities/1").await.1["result"]["time_s"],
        5400.0
    );

    assert_eq!(
        send("DELETE", 1, Value::Null, cookie).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(&pool, "/api/activities/1").await.1["result"],
        Value::Null
    );
}
