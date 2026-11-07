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

#[tokio::test]
async fn counts_distinct_runs_per_cell() {
    let pool = common::test_pool().await;
    // Run 1 lingers in one spot: repeated points must still count as one run.
    add_run(&pool, 1, &[BERLIN, BERLIN, BERLIN]).await;
    add_run(&pool, 2, &[BERLIN, SYDNEY]).await;

    let (status, body) = get(&pool, &format!("/api/heatmap?zoom=10&{WORLD}")).await;
    assert_eq!(status, StatusCode::OK);
    let mut cells = body["cells"].as_array().unwrap().clone();
    cells.sort_by_key(|cell| cell["runs"].as_i64());
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[0]["runs"], 1);
    assert_eq!(cells[1]["runs"], 2);

    // Cell centers must land close to the real position.
    assert!((cells[1]["lat"].as_f64().unwrap() - BERLIN.lat).abs() < 0.01);
    assert!((cells[1]["lon"].as_f64().unwrap() - BERLIN.lon).abs() < 0.01);
    assert!((cells[0]["lat"].as_f64().unwrap() - SYDNEY.lat).abs() < 0.01);
    assert!((cells[0]["lon"].as_f64().unwrap() - SYDNEY.lon).abs() < 0.01);
}

#[tokio::test]
async fn viewport_excludes_other_points() {
    let pool = common::test_pool().await;
    add_run(&pool, 1, &[BERLIN, SYDNEY]).await;

    let (_, body) = get(
        &pool,
        "/api/heatmap?zoom=5&west=0&south=40&east=30&north=60",
    )
    .await;
    let cells = body["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 1);
    assert!((cells[0]["lat"].as_f64().unwrap() - BERLIN.lat).abs() < 2.0);
}

#[tokio::test]
async fn lower_zoom_merges_nearby_points() {
    let pool = common::test_pool().await;
    let near = Point {
        lat: 52.5201,
        lon: 13.4001,
    };
    add_run(&pool, 1, &[BERLIN]).await;
    add_run(&pool, 2, &[near]).await;

    let (_, low) = get(&pool, &format!("/api/heatmap?zoom=3&{WORLD}")).await;
    assert_eq!(low["cells"].as_array().unwrap().len(), 1);
    assert_eq!(low["cells"][0]["runs"], 2);
}

#[tokio::test]
async fn grid_stops_getting_finer_at_the_maximum_zoom() {
    let pool = common::test_pool().await;
    add_run(&pool, 1, &[BERLIN]).await;

    let (_, requested) = get(&pool, &format!("/api/heatmap?zoom=12&{WORLD}")).await;
    assert_eq!(requested["grid_zoom"], 12);
    let (_, finest) = get(&pool, &format!("/api/heatmap?zoom=18&{WORLD}")).await;
    let (_, capped) = get(&pool, &format!("/api/heatmap?zoom=22&{WORLD}")).await;
    assert_eq!(capped["grid_zoom"], 18);
    assert_eq!(capped["cells"], finest["cells"]);
}

/// Cells of a run along a west-east line from `BERLIN`, which spans `meters`.
async fn cells_on_line(pool: &Pool, meters: f64, zoom: u8) -> Vec<Value> {
    let metres_per_degree = 111_320.0 * BERLIN.lat.to_radians().cos();
    let end = Point {
        lat: BERLIN.lat,
        lon: BERLIN.lon + meters / metres_per_degree,
    };
    add_run(pool, 1, &[BERLIN, end]).await;
    let (_, body) = get(pool, &format!("/api/heatmap?zoom={zoom}&{WORLD}")).await;
    body["cells"].as_array().unwrap().clone()
}

#[tokio::test]
async fn connects_consecutive_points() {
    let pool = common::test_pool().await;
    // A cell at zoom 18 is 0.18 m wide at this latitude, so 50 m spans about 270.
    let cells = cells_on_line(&pool, 50.0, 18).await;
    assert!((260..=280).contains(&cells.len()), "{}", cells.len());
    assert!(cells.iter().all(|cell| cell["runs"] == 1));
}

#[tokio::test]
async fn does_not_connect_across_long_gaps() {
    let pool = common::test_pool().await;
    let cells = cells_on_line(&pool, 150.0, 18).await;
    assert_eq!(cells.len(), 2);
}

#[tokio::test]
async fn does_not_connect_points_on_both_sides_of_the_viewport_edge() {
    let pool = common::test_pool().await;
    let east = |meters: f64| Point {
        lat: BERLIN.lat,
        lon: BERLIN.lon + meters / (111_320.0 * BERLIN.lat.to_radians().cos()),
    };
    // Leaves the viewport in the middle: the outer points are not consecutive.
    add_run(&pool, 1, &[BERLIN, east(20.0), east(40.0)]).await;
    let south = BERLIN.lat - 0.001;
    let north = BERLIN.lat + 0.001;
    let middle = BERLIN.lon + 10.0 / (111_320.0 * BERLIN.lat.to_radians().cos());
    let (_, body) = get(
        &pool,
        &format!("/api/heatmap?zoom=18&west=0&south={south}&east={middle}&north={north}"),
    )
    .await;
    assert_eq!(body["cells"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn connected_runs_count_once_per_cell() {
    let pool = common::test_pool().await;
    let east = Point {
        lat: BERLIN.lat,
        lon: BERLIN.lon + 0.0002,
    };
    add_run(&pool, 1, &[BERLIN, east, BERLIN]).await;
    add_run(&pool, 2, &[BERLIN, east]).await;

    let (_, body) = get(&pool, &format!("/api/heatmap?zoom=18&{WORLD}")).await;
    let cells = body["cells"].as_array().unwrap();
    assert!(cells.len() > 10);
    assert!(cells.iter().all(|cell| cell["runs"] == 2), "{cells:?}");
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
    let fit = common::fit_of(&[(
        common::semicircles(BERLIN.lat),
        common::semicircles(BERLIN.lon),
    )]);
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
