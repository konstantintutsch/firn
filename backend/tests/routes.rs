mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use diesel_async::RunQueryDsl;
use http_body_util::BodyExt;
use runmap::api::router;
use runmap::db::{self, Pool};
use runmap::track::Point;
use serde_json::{Value, json};
use tower::ServiceExt;

const GPX: &str = r#"<?xml version="1.0"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk><trkseg>
    <trkpt lat="52.5200" lon="13.4000"><ele>34</ele></trkpt>
    <trkpt lat="52.5300" lon="13.4000"><ele>40</ele></trkpt>
    <trkpt lat="52.5400" lon="13.4000"><ele>38</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn call(
    pool: &Pool,
    cookie: Option<&str>,
    method: &str,
    uri: &str,
    header: bool,
    body: Body,
) -> Reply {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if header {
        request = request.header("x-requested-with", "runmap");
    }
    let response = router(common::state(pool.clone(), "http://unused", None).await)
        .oneshot(
            request
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    Reply {
        status: response.status(),
        body: response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    }
}

async fn send(pool: &Pool, method: &str, uri: &str, body: Value) -> Reply {
    let cookie = common::cookie(pool).await;
    call(
        pool,
        Some(&cookie),
        method,
        uri,
        true,
        Body::from(body.to_string()),
    )
    .await
}

async fn import(pool: &Pool, uri: &str, gpx: &str) -> Reply {
    let cookie = common::cookie(pool).await;
    call(
        pool,
        Some(&cookie),
        "POST",
        uri,
        true,
        Body::from(gpx.to_owned()),
    )
    .await
}

async fn get(pool: &Pool, uri: &str) -> Reply {
    let cookie = common::cookie(pool).await;
    call(pool, Some(&cookie), "GET", uri, false, Body::empty()).await
}

async fn add_run_with_file(pool: &Pool, id: i64, gpx: Option<&str>) {
    let activity = runmap::garmin::Activity {
        id,
        name: "Morning loop".into(),
        activity_type: "running".into(),
        started_at: chrono::Utc::now(),
        distance_m: Some(1000.0),
        duration_s: None,
        has_track: true,
        summary: json!({"activityId": id, "startTimeGMT": "2026-03-01 08:00:00"}),
    };
    db::upsert_activities(pool, common::user(pool).await, &[activity])
        .await
        .unwrap();
    db::save_track(
        pool,
        id,
        &[Point {
            lat: 52.52,
            lon: 13.40,
        }],
    )
    .await
    .unwrap();
    if let Some(gpx) = gpx {
        let zip = common::zip_of(&[("1.gpx", gpx.as_bytes())]);
        common::files(pool)
            .await
            .save_original(id, Some(&zip))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn imports_a_gpx_file_as_a_route() {
    let pool = common::test_pool().await;
    let reply = import(&pool, "/api/routes/import?name=%20Canal%20", GPX).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let route = reply.json();
    assert_eq!(route["name"], "Canal");
    assert_eq!(route["activity_id"], Value::Null);
    // Two steps of 0.01 degrees of latitude.
    assert!((route["distance_m"].as_f64().unwrap() - 2223.9).abs() < 5.0);
    // Up 6 m is above the noise. Down again does not count.
    assert_eq!(route["elevation_gain_m"], 6.0);

    let detail = get(
        &pool,
        &format!("/api/routes/{}", route["id"].as_str().unwrap()),
    )
    .await
    .json();
    assert_eq!(detail["points"].as_array().unwrap().len(), 3);
    assert_eq!(detail["points"][1], json!([52.53, 13.4, 40.0]));
}

#[tokio::test]
async fn rejects_files_without_a_track() {
    let pool = common::test_pool().await;
    for gpx in [
        "not xml at all",
        "<gpx></gpx>",
        r#"<gpx><trk><trkseg><trkpt lat="1" lon="1"/></trkseg></trk></gpx>"#,
    ] {
        let reply = import(&pool, "/api/routes/import", gpx).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{gpx}");
    }
    let routes = get(&pool, "/api/routes").await.json();
    assert_eq!(routes["routes"], json!([]));
}

#[tokio::test]
async fn takes_route_points_when_a_file_has_no_track() {
    let pool = common::test_pool().await;
    let gpx =
        r#"<gpx><rte><rtept lat="52.5" lon="13.4"/><rtept lat="52.6" lon="13.4"/></rte></gpx>"#;
    let reply = import(&pool, "/api/routes/import", gpx).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.json()["name"], "Imported route");
    assert_eq!(reply.json()["elevation_gain_m"], Value::Null);
}

#[tokio::test]
async fn saves_an_activity_as_a_route() {
    let pool = common::test_pool().await;
    add_run_with_file(&pool, 1, Some(GPX)).await;
    let reply = send(&pool, "POST", "/api/routes", json!({"activity_id": 1})).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let route = reply.json();
    assert_eq!(route["name"], "Morning loop");
    assert_eq!(route["activity_id"], 1);

    let named = send(
        &pool,
        "POST",
        "/api/routes",
        json!({"activity_id": 1, "name": "Sunday", "description": " Flat "}),
    )
    .await
    .json();
    assert_eq!(named["name"], "Sunday");
    assert_eq!(named["description"], "Flat");

    // Newest first.
    let list = get(&pool, "/api/routes").await.json();
    assert_eq!(list["routes"][0]["name"], "Sunday");
    assert_eq!(list["routes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_activity_without_a_file_or_an_owner_makes_no_route() {
    let pool = common::test_pool().await;
    add_run_with_file(&pool, 1, None).await;
    let reply = send(&pool, "POST", "/api/routes", json!({"activity_id": 1})).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    let reply = send(&pool, "POST", "/api/routes", json!({"activity_id": 2})).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renames_and_deletes_a_route() {
    let pool = common::test_pool().await;
    let id = import(&pool, "/api/routes/import", GPX).await.json()["id"].clone();
    let uri = format!("/api/routes/{}", id.as_str().unwrap());

    let reply = send(
        &pool,
        "PUT",
        &uri,
        json!({"name": "Canal", "description": "Along the water"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let route = get(&pool, &uri).await.json();
    assert_eq!(route["name"], "Canal");
    assert_eq!(route["description"], "Along the water");

    let reply = send(&pool, "PUT", &uri, json!({"name": "  "})).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    assert_eq!(
        send(&pool, "DELETE", &uri, json!(null)).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(get(&pool, &uri).await.status, StatusCode::NOT_FOUND);
    let reply = send(&pool, "PUT", &uri, json!({"name": "Gone"})).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deleting_the_activity_keeps_the_route() {
    let pool = common::test_pool().await;
    add_run_with_file(&pool, 1, Some(GPX)).await;
    let id = send(&pool, "POST", "/api/routes", json!({"activity_id": 1}))
        .await
        .json()["id"]
        .clone();
    diesel::sql_query("DELETE FROM activities WHERE id = 1")
        .execute(&mut pool.get().await.unwrap())
        .await
        .unwrap();
    let route = get(&pool, &format!("/api/routes/{}", id.as_str().unwrap()))
        .await
        .json();
    assert_eq!(route["activity_id"], Value::Null);
}

#[tokio::test]
async fn downloads_a_route_as_gpx() {
    let pool = common::test_pool().await;
    let id = import(&pool, "/api/routes/import?name=Canal", GPX)
        .await
        .json()["id"]
        .clone();
    let reply = get(&pool, &format!("/api/routes/{}/file", id.as_str().unwrap())).await;
    assert_eq!(reply.status, StatusCode::OK);
    let gpx = String::from_utf8(reply.body).unwrap();
    assert!(gpx.contains("<name>Canal</name>"));
    assert_eq!(gpx.matches("<trkpt").count(), 3);
    assert!(gpx.contains("<ele>40.0</ele>"));
}

#[tokio::test]
async fn accounts_see_only_their_own_routes() {
    let pool = common::test_pool().await;
    let id = import(&pool, "/api/routes/import", GPX).await.json()["id"].clone();
    let other = common::user_named(&pool, "other").await;
    let cookie = common::cookie_for(&pool, other).await;
    let uri = format!("/api/routes/{}", id.as_str().unwrap());

    let list = call(
        &pool,
        Some(&cookie),
        "GET",
        "/api/routes",
        false,
        Body::empty(),
    )
    .await;
    assert_eq!(list.json()["routes"], json!([]));
    for method in ["GET", "DELETE"] {
        let reply = call(&pool, Some(&cookie), method, &uri, true, Body::empty()).await;
        assert_ne!(reply.status, StatusCode::OK, "{method}");
    }
    let reply = call(
        &pool,
        Some(&cookie),
        "GET",
        &format!("{uri}/file"),
        false,
        Body::empty(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    // The delete changed nothing.
    assert_eq!(get(&pool, &uri).await.status, StatusCode::OK);
}

#[tokio::test]
async fn changes_need_the_header_and_a_session() {
    let pool = common::test_pool().await;
    let cookie = common::cookie(&pool).await;
    let reply = call(
        &pool,
        Some(&cookie),
        "POST",
        "/api/routes/import",
        false,
        Body::from(GPX),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let reply = call(&pool, None, "GET", "/api/routes", false, Body::empty()).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn routes_have_random_ids_and_a_small_preview() {
    let pool = common::test_pool().await;
    let first = import(&pool, "/api/routes/import", GPX).await.json();
    let second = import(&pool, "/api/routes/import", GPX).await.json();
    let id = |route: &Value| route["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap();
    assert_ne!(id(&first), id(&second));
    assert_eq!(
        first["preview"],
        json!([[52.52, 13.4], [52.53, 13.4], [52.54, 13.4]])
    );

    let reply = get(&pool, "/api/routes/1").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}
