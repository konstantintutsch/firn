mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use runmap::api::{AppState, router};
use runmap::db::{self, Pool};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GPX: &[u8] = br#"<gpx><trk><type>running</type><trkseg>
  <trkpt lat="52.50" lon="13.40"><ele>30</ele><time>2026-05-01T07:00:00Z</time></trkpt>
  <trkpt lat="52.51" lon="13.40"><ele>34</ele><time>2026-05-01T07:05:00Z</time></trkpt>
</trkseg></trk></gpx>"#;

async fn send(
    state: &AppState,
    request: axum::http::request::Builder,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let request = request
        .header("cookie", common::cookie(state.pool()).await)
        .header("x-requested-with", "runmap");
    let response = router(state.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

async fn import(state: &AppState, zip: Vec<u8>) -> (StatusCode, Value) {
    let (status, body) = send(state, Request::post("/api/import"), Body::from(zip)).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The Garmin IDs of the test account's activities, oldest first.
async fn activity_ids(pool: &Pool) -> Vec<i64> {
    let user_id = common::user(pool).await;
    let ids = db::activity_ids(pool, user_id).await.unwrap();
    ids_to_garmin(pool, &ids).await
}

async fn ids_to_garmin(pool: &Pool, ids: &[uuid::Uuid]) -> Vec<i64> {
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use runmap::schema::activities;
    let mut garmin_ids = Vec::new();
    for id in ids {
        garmin_ids.push(
            activities::table
                .find(id)
                .select(activities::garmin_id)
                .first(&mut pool.get().await.unwrap())
                .await
                .unwrap(),
        );
    }
    garmin_ids
}

/// Syncs one activity with a photo and a day of health data from a fake Garmin.
async fn synced_state() -> AppState {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/personalapitoken-service/personal-api/personal-api-key/token",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string("access"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/activitylist-service/activities/search/activities"))
        .and(wiremock::matchers::query_param("start", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "activityId": 7,
            "activityName": "Lunch run",
            "startTimeGMT": "2026-05-01 12:00:00",
            "activityType": {"typeKey": "running"},
            "distance": 1000.0,
            "startLatitude": 52.5,
        }])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/activitylist-service/activities/search/activities"))
        .and(wiremock::matchers::query_param("start", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let fit = common::fit_of(&[
        (common::semicircles(52.5), common::semicircles(13.4)),
        (common::semicircles(52.6), common::semicircles(13.5)),
    ]);
    Mock::given(method("GET"))
        .and(path("/download-service/files/activity/7"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(common::zip_of(&[("7.fit", &fit)])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/activity-service/activity/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadataDTO": {"activityImages": [
                {"imageId": "p1", "url": format!("{}/photo.jpg", server.uri())}
            ]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/photo.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"jpeg".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sleep-service/sleep/dailySleep"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "dailySleepDTO": {"calendarDate": "2026-05-01", "sleepTimeSeconds": 27000}
        })))
        .mount(&server)
        .await;

    let pool = common::test_pool().await;
    let state = common::state(pool, &server.uri(), Some(common::API_KEY)).await;
    send(&state, Request::post("/api/sync"), Body::empty()).await;
    for _ in 0..500 {
        let (_, body) = send(&state, Request::get("/api/sync"), Body::empty()).await;
        let status: Value = serde_json::from_slice(&body).unwrap();
        if status["state"] != "running" {
            assert_eq!(status["state"], "succeeded", "{status}");
            return state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the sync did not finish");
}

#[tokio::test]
async fn an_export_restores_everything_in_another_runmap() {
    let source = synced_state().await;
    let id = common::activity_id(source.pool(), 7).await;
    let result = json!({"distance_m": 1000.0, "time_s": 240.0, "event": null, "results_url": null});
    let request = Request::put(format!("/api/activities/{}/result", id))
        .header("content-type", "application/json");
    let (status, _) = send(&source, request, Body::from(result.to_string())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let route = json!({"activity_id": id});
    let request = Request::post("/api/routes").header("content-type", "application/json");
    let (status, body) = send(&source, request, Body::from(route.to_string())).await;
    assert_eq!(status, StatusCode::CREATED);
    let route: Value = serde_json::from_slice(&body).unwrap();
    let (status, zip) = send(&source, Request::get("/api/export"), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);

    let pool = common::test_pool().await;
    let target = common::state(pool.clone(), "http://unused", None).await;
    let (status, report) = import(&target, zip.clone()).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["activities"], 1);
    assert_eq!(report["failed"], 0);
    assert_eq!(report["routes"], 1);
    assert!(report["health_documents"].as_u64().unwrap() > 0);

    assert_eq!(activity_ids(&pool).await, [7]);
    let files = common::files(&pool).await;
    assert!(files.original(id).await.unwrap().is_some());
    assert_eq!(files.image(id, "p1").await.unwrap(), Some(b"jpeg".to_vec()));
    let (_, body) = send(
        &target,
        Request::get(format!("/api/activities/{}", id)),
        Body::empty(),
    )
    .await;
    let detail: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail["activity"]["name"], "Lunch run");
    assert_eq!(detail["result"], result);
    assert_eq!(
        detail["images"],
        json!([format!("/api/activities/{id}/images/p1")])
    );
    assert_eq!(detail["samples"]["position"].as_array().unwrap().len(), 2);
    let (_, body) = send(
        &target,
        Request::get("/api/statistics/health?from=2026-05-01&to=2026-05-01"),
        Body::empty(),
    )
    .await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["metrics"]["sleep_h"][0][1], 7.5);

    // The route keeps its ID, its activity and its points.
    let uri = format!("/api/routes/{}", route["id"].as_str().unwrap());
    let (status, body) = send(&target, Request::get(&uri), Body::empty()).await;
    assert_eq!(status, StatusCode::OK);
    let imported: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(imported["id"], route["id"]);
    assert_eq!(imported["activity_id"], id.to_string());
    assert_eq!(imported["created_at"], route["created_at"]);
    assert_eq!(imported["points"].as_array().unwrap().len(), 2);
    assert_eq!(imported["preview"].as_array().unwrap().len(), 2);

    // Importing again changes nothing, and another account cannot take the activity or the route.
    assert_eq!(import(&target, zip.clone()).await.1["activities"], 1);
    assert_eq!(activity_ids(&pool).await, [7]);
    let (_, body) = send(&target, Request::get("/api/routes"), Body::empty()).await;
    let list: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["routes"].as_array().unwrap().len(), 1);
    let other = common::user_named(&pool, "other").await;
    let request = Request::post("/api/import")
        .header("cookie", common::cookie_for(&pool, other).await)
        .header("x-requested-with", "runmap");
    let response = router(target.clone())
        .oneshot(request.body(Body::from(zip)).unwrap())
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        (report["activities"].clone(), report["skipped"].clone()),
        (json!(0), json!(2))
    );
}

#[tokio::test]
async fn imports_loose_files_once() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    let no_time = br#"<gpx><trk><trkseg><trkpt lat="1" lon="2"/></trkseg></trk></gpx>"#;
    let zip = common::zip_of(&[
        ("runs/Morning.gpx", GPX),
        ("broken.gpx", no_time),
        ("notes.txt", b"hi"),
    ]);

    let (status, report) = import(&state, zip.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        report,
        json!({"activities": 1, "skipped": 0, "failed": 1, "health_documents": 0, "routes": 0})
    );
    let ids = activity_ids(&pool).await;
    assert_eq!(ids.len(), 1);
    assert!(ids[0] < 0);
    let id = common::activity_id(&pool, ids[0]).await;

    import(&state, zip).await;
    assert_eq!(activity_ids(&pool).await, ids);

    let (_, body) = send(
        &state,
        Request::get(format!("/api/activities/{id}")),
        Body::empty(),
    )
    .await;
    let detail: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail["activity"]["name"], "Morning");
    assert_eq!(detail["activity"]["activity_type"], "running");
    assert_eq!(detail["activity"]["duration_s"], 300.0);
    assert!((detail["activity"]["distance_m"].as_f64().unwrap() - 1112.0).abs() < 5.0);
    assert_eq!(detail["summary"]["elevationGain"], 4.0);
}

#[tokio::test]
async fn import_refuses_what_is_not_a_zip() {
    let pool = common::test_pool().await;
    let state = common::state(pool, "http://unused", None).await;
    let (status, _) = import(&state, b"not a zip".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
