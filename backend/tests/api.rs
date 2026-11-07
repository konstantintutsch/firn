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
    let response = router(common::state(pool.clone(), "http://unused", None))
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
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
    db::upsert_runs(pool, &[activity]).await.unwrap();
    db::save_track(pool, id, Some(b"zip".as_slice()), points)
        .await
        .unwrap();
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
    assert_eq!(empty["runs"], 0);
    assert_eq!(empty["distance_m"], 0.0);

    add_run(&pool, 1, &[BERLIN]).await;
    add_run(&pool, 2, &[]).await;
    let (_, body) = get(&pool, "/api/stats").await;
    assert_eq!(body["runs"], 2);
    assert_eq!(body["distance_m"], 2000.0);
}

#[tokio::test]
async fn publishes_openapi_without_secrets() {
    let pool = common::test_pool().await;
    let (status, spec) = get(&pool, "/api/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(spec["paths"]["/api/heatmap"].is_object());
    assert!(spec["paths"]["/api/stats"].is_object());
}

async fn request_sync(state: &AppState, with_header: bool) -> (StatusCode, Value) {
    let mut request = Request::post("/api/sync");
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
    let response = router(state.clone())
        .oneshot(Request::get("/api/sync").body(Body::empty()).unwrap())
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
    let state = common::state(pool.clone(), &server.uri(), Some(common::API_KEY));
    assert_eq!(sync_status(&state).await["state"], "idle");

    let (status, body) = request_sync(&state, true).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["state"], "running");

    let finished = finished_sync(&state).await;
    assert_eq!(finished["state"], "succeeded");
    assert_eq!(finished["listed"], 1);
    assert_eq!(finished["to_download"], 1);
    assert_eq!(finished["downloaded"], 1);
    assert_eq!(db::totals(&pool).await.unwrap().runs, 1);

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
    let state = common::state(pool, &server.uri(), Some(common::API_KEY));

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
    let state = common::state(pool, &server.uri(), Some(common::API_KEY));

    assert_eq!(request_sync(&state, true).await.0, StatusCode::ACCEPTED);
    assert_eq!(request_sync(&state, true).await.0, StatusCode::CONFLICT);
    assert_eq!(finished_sync(&state).await["state"], "succeeded");
}

#[tokio::test]
async fn sync_without_a_key_explains_what_to_do() {
    let pool = common::test_pool().await;
    let state = common::state(pool, "http://unused", None);

    let (status, body) = request_sync(&state, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("GARMIN_PERSONAL_API_KEY")
    );
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
    let state = common::state(pool, &server.uri(), Some(common::API_KEY));

    request_sync(&state, true).await;
    let finished = finished_sync(&state).await;
    assert_eq!(finished["state"], "failed");
    let error = finished["error"].as_str().unwrap();
    assert!(error.contains("refused the API key"));
    assert!(!finished.to_string().contains(common::API_KEY));
}

#[tokio::test]
async fn map_shows_new_runs_after_a_sync() {
    let server = MockServer::start().await;
    mount_garmin(&server, Duration::ZERO).await;
    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY));
    let lines = async |state: &AppState| {
        let uri = format!("/api/heatmap?zoom=10&{WORLD}");
        let response = router(state.clone())
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
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
