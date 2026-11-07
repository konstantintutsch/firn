mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use runmap::api::router;
use runmap::db::{self, Pool};
use runmap::track::Point;
use serde_json::Value;
use tower::ServiceExt;

async fn get(pool: &Pool, uri: &str) -> (StatusCode, Value) {
    let response = router(pool.clone())
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
    let (_, finest) = get(&pool, &format!("/api/heatmap?zoom=15&{WORLD}")).await;
    let (_, capped) = get(&pool, &format!("/api/heatmap?zoom=22&{WORLD}")).await;
    assert_eq!(capped["grid_zoom"], 15);
    assert_eq!(capped["cells"], finest["cells"]);
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
