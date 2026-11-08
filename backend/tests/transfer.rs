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
    let result = json!({"distance_m": 1000.0, "time_s": 240.0, "event": null, "results_url": null, "target_time_s": 230.0});
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
        json!({
            "activities": 1, "skipped": 0, "failed": 1, "health_documents": 0, "routes": 0,
            "removed_activities": 0, "removed_routes": 0, "removed_health_documents": 0,
            "replace_skipped": false,
        })
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

async fn replace(state: &AppState, zip: Vec<u8>) -> Value {
    let request = Request::post("/api/import?replace=true");
    let (status, body) = send(state, request, Body::from(zip)).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn a_replacing_import_leaves_only_what_it_holds() {
    let source = synced_state().await;
    let id = common::activity_id(source.pool(), 7).await;
    let (_, export) = send(&source, Request::get("/api/export"), Body::empty()).await;

    // The account has a loose activity, a route and a day of health data of its own.
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    import(&state, common::zip_of(&[("Morning.gpx", GPX)])).await;
    let loose = activity_ids(&pool).await[0];
    let loose_id = common::activity_id(&pool, loose).await;
    let request = Request::post("/api/routes").header("content-type", "application/json");
    let route = json!({"activity_id": loose_id}).to_string();
    let (status, _) = send(&state, request, Body::from(route)).await;
    assert_eq!(status, StatusCode::CREATED);
    let user = common::user(&pool).await;
    db::save_document(
        &pool,
        user,
        "sleep",
        "2020-01-01",
        Some(&json!({})),
        &[("2020-01-01".parse().unwrap(), "sleep_h", 6.0)],
    )
    .await
    .unwrap();

    // Without replacing, the import adds to them.
    import(&state, export.clone()).await;
    assert_eq!(activity_ids(&pool).await.len(), 2);

    let report = replace(&state, export).await;
    assert_eq!(report["removed_activities"], 1);
    assert_eq!(report["removed_routes"], 1);
    assert!(report["removed_health_documents"].as_u64().unwrap() >= 1);
    assert_eq!(report["replace_skipped"], false);
    assert_eq!(activity_ids(&pool).await, [7]);
    let files = common::files(&pool).await;
    assert!(files.original(loose_id).await.unwrap().is_none());
    assert!(files.original(id).await.unwrap().is_some());
    let (_, body) = send(&state, Request::get("/api/routes"), Body::empty()).await;
    let list: Value = serde_json::from_slice(&body).unwrap();
    assert!(list["routes"].as_array().unwrap().is_empty());
    let (_, body) = send(
        &state,
        Request::get("/api/statistics/health"),
        Body::empty(),
    )
    .await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["metrics"]["sleep_h"].as_array().unwrap().len(), 1);
    assert_eq!(health["metrics"]["sleep_h"][0][0], "2026-05-01");
}

#[tokio::test]
async fn a_replacing_import_with_unreadable_files_removes_nothing() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    import(&state, common::zip_of(&[("Morning.gpx", GPX)])).await;
    let before = activity_ids(&pool).await;
    let no_time = br#"<gpx><trk><trkseg><trkpt lat="1" lon="2"/></trkseg></trk></gpx>"#;

    let report = replace(&state, common::zip_of(&[("broken.gpx", no_time)])).await;
    assert_eq!(report["failed"], 1);
    assert_eq!(report["replace_skipped"], true);
    assert_eq!(report["removed_activities"], 0);
    assert_eq!(activity_ids(&pool).await, before);
}

const APPLE_EXPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<HealthData locale="en_DE">
 <Record type="HKQuantityTypeIdentifierStepCount" unit="count" value="4000" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 09:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierHeartRate" unit="count/min" value="61" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 08:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierVO2Max" unit="mL/min·kg" value="49.5" sourceName="Watch" startDate="2026-04-10 08:31:00 +0200" endDate="2026-04-10 08:31:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierVO2Max" unit="mL/min·kg" value="49.7" sourceName="Watch" startDate="2026-04-10 19:00:00 +0200" endDate="2026-04-10 19:00:00 +0200"/>
 <Record type="HKQuantityTypeIdentifierDietaryWater" unit="mL" value="500" sourceName="Phone" startDate="2026-04-10 12:00:00 +0200" endDate="2026-04-10 12:00:00 +0200"/>
 <Workout workoutActivityType="HKWorkoutActivityTypeRunning" duration="30" durationUnit="min" totalDistance="5" totalDistanceUnit="km" totalEnergyBurned="350" sourceName="Watch" startDate="2026-04-10 08:00:00 +0200" endDate="2026-04-10 08:30:00 +0200">
  <WorkoutRoute sourceName="Watch"><FileReference path="/workout-routes/route_1.gpx"/></WorkoutRoute>
 </Workout>
 <Workout workoutActivityType="HKWorkoutActivityTypeTraditionalStrengthTraining" duration="45" durationUnit="min" sourceName="Watch" startDate="2026-04-11 18:00:00 +0200" endDate="2026-04-11 18:45:00 +0200"/>
</HealthData>"#;

fn apple_zip() -> Vec<u8> {
    common::zip_of(&[
        ("apple_health_export/Export.xml", APPLE_EXPORT.as_bytes()),
        ("apple_health_export/workout-routes/route_1.gpx", GPX),
    ])
}

async fn import_apple(state: &AppState, scope: &str, replace: bool) -> (StatusCode, Value) {
    let uri = format!("/api/import?apple_health={scope}&replace={replace}");
    let (status, body) = send(state, Request::post(uri), Body::from(apple_zip())).await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn imports_only_the_health_data_of_apple_health() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;

    let (status, report) = import_apple(&state, "health", false).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["health_documents"], 1);
    assert_eq!(report["activities"], 0);
    assert!(activity_ids(&pool).await.is_empty());
    let (_, body) = send(
        &state,
        Request::get("/api/statistics/health"),
        Body::empty(),
    )
    .await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["metrics"]["steps"][0], json!(["2026-04-10", 4000.0]));
    assert_eq!(health["metrics"]["min_heart_rate"][0][1], 61.0);
}

#[tokio::test]
async fn imports_the_workouts_of_apple_health_once() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;

    let (status, report) = import_apple(&state, "workouts", false).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        (report["activities"].clone(), report["failed"].clone()),
        (json!(2), json!(0))
    );
    let (_, again) = import_apple(&state, "workouts", false).await;
    assert_eq!(again["activities"], 2);
    let ids = activity_ids(&pool).await;
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| *id < 0));

    let (_, body) = send(
        &state,
        Request::get("/api/activities?without_gps=true&sort=oldest"),
        Body::empty(),
    )
    .await;
    let list: Value = serde_json::from_slice(&body).unwrap();
    let [run, gym] = &list["activities"].as_array().unwrap()[..] else {
        panic!("{list}");
    };
    assert_eq!(run["activity_type"], "running");
    assert_eq!(run["distance_m"], 5000.0);
    assert_eq!(run["duration_s"], 1800.0);
    assert_eq!(run["name"], "Running");
    assert!(run["bounds"].is_array());
    assert_eq!(gym["activity_type"], "strength_training");
    assert_eq!(gym["bounds"], Value::Null);
    let (_, body) = send(
        &state,
        Request::get(format!("/api/activities/{}", run["id"].as_str().unwrap())),
        Body::empty(),
    )
    .await;
    let detail: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detail["summary"]["calories"], 350.0);
    assert_eq!(detail["summary"]["startTimeLocal"], "2026-04-10 08:00:00");
    assert_eq!(detail["summary"]["startTimeGMT"], "2026-04-10 06:00:00");
}

#[tokio::test]
async fn a_zip_without_apple_health_data_is_refused() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    let zip = common::zip_of(&[("notes.txt", b"hi")]);
    let request = Request::post("/api/import?apple_health=health");
    let (status, _) = send(&state, request, Body::from(zip)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn replacing_with_apple_health_removes_the_rest() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;
    import(&state, common::zip_of(&[("Morning.gpx", GPX)])).await;
    assert_eq!(activity_ids(&pool).await.len(), 1);

    let (_, report) = import_apple(&state, "health", true).await;
    assert_eq!(report["removed_activities"], 1);
    assert!(activity_ids(&pool).await.is_empty());
}

#[tokio::test]
async fn an_export_carries_the_health_data_of_apple_health() {
    let source_pool = common::test_pool().await;
    let source = common::state(source_pool.clone(), "http://unused", None).await;
    import_apple(&source, "health", false).await;
    let (_, zip) = send(&source, Request::get("/api/export"), Body::empty()).await;

    let pool = common::test_pool().await;
    let target = common::state(pool.clone(), "http://unused", None).await;
    let (status, report) = import(&target, zip).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["health_documents"], 1);
    let (_, body) = send(
        &target,
        Request::get("/api/statistics/health"),
        Body::empty(),
    )
    .await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["metrics"]["steps"][0], json!(["2026-04-10", 4000.0]));
}

/// The VO2 max values of the test account: their number, value and activity.
async fn vo2_max_rows(pool: &Pool) -> Vec<(i32, f64, Option<uuid::Uuid>)> {
    use diesel::prelude::*;
    use diesel_async::RunQueryDsl;
    use runmap::schema::daily_metrics;
    let user = common::user(pool).await;
    daily_metrics::table
        .filter(daily_metrics::user_id.eq(user))
        .filter(daily_metrics::metric.eq("vo2_max"))
        .order((daily_metrics::day, daily_metrics::seq))
        .select((
            daily_metrics::seq,
            daily_metrics::value,
            daily_metrics::activity_id,
        ))
        .load(&mut pool.get().await.unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn keeps_every_vo2_max_of_a_day_and_links_it_to_its_workout() {
    let pool = common::test_pool().await;
    let state = common::state(pool.clone(), "http://unused", None).await;

    // Without the workouts, there is nothing to link to.
    import_apple(&state, "health", false).await;
    assert_eq!(
        vo2_max_rows(&pool).await,
        [(0, 49.5, None), (1, 49.7, None)]
    );

    // The workouts, and the VO2 max after the run is the run's. The evening's is not.
    import_apple(&state, "workouts", false).await;
    let ids = activity_ids(&pool).await;
    let run = {
        let (_, body) = send(
            &state,
            Request::get("/api/activities?without_gps=true&sort=oldest"),
            Body::empty(),
        )
        .await;
        let list: Value = serde_json::from_slice(&body).unwrap();
        list["activities"][0]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert_eq!(ids.len(), 2);
    assert_eq!(
        vo2_max_rows(&pool).await,
        [(0, 49.5, Some(run)), (1, 49.7, None)]
    );

    // The health data alone links to workouts that were imported before.
    import_apple(&state, "health", false).await;
    assert_eq!(vo2_max_rows(&pool).await[0].2, Some(run));

    // Both values are in the statistics, and an export carries them with their link.
    let (_, body) = send(
        &state,
        Request::get("/api/statistics/health"),
        Body::empty(),
    )
    .await;
    let health: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(health["metrics"]["vo2_max"].as_array().unwrap().len(), 2);
    assert!(health["metrics"].get("hydration_l").is_none());
    let (_, zip) = send(&state, Request::get("/api/export"), Body::empty()).await;
    let other_pool = common::test_pool().await;
    let other = common::state(other_pool.clone(), "http://unused", None).await;
    import(&other, zip).await;
    assert_eq!(
        vo2_max_rows(&other_pool).await,
        [(0, 49.5, Some(run)), (1, 49.7, None)]
    );

    // Removing the workout leaves the value, without its link.
    let user = common::user(&pool).await;
    runmap::db::delete_activities(&pool, user, &[run])
        .await
        .unwrap();
    assert_eq!(
        vo2_max_rows(&pool).await,
        [(0, 49.5, None), (1, 49.7, None)]
    );
}
