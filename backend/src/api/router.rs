use axum::Router;
use axum::routing::{get, post, put};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use super::{
    activities, activity, activity_types, archive, file, health, heatmap, results, stats, sync,
    training_blocks,
};
use crate::authentication::{self};
use crate::routes;
use crate::state::AppState;
use crate::statistics;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "firn",
        description = "Heatmap of activities saved from Garmin Connect"
    ),
    paths(
        heatmap::heatmap,
        activities::activities,
        health::health,
        activity::activity,
        file::activity_file,
        activity::resync_activity,
        results::save_result,
        results::delete_result,
        file::activity_image,
        archive::export,
        archive::import,
        activity_types::activity_types,
        training_blocks::training_blocks,
        routes::list_routes,
        routes::create_route,
        routes::import_route,
        routes::route,
        routes::update_route,
        routes::delete_route,
        routes::route_file,
        stats::stats,
        sync::sync_status,
        sync::start_sync,
        sync::cancel_sync,
        authentication::login,
        authentication::callback,
        authentication::logout,
        authentication::me,
        authentication::delete_me,
        authentication::set_garmin_key,
        authentication::delete_garmin_key,
        statistics::activity_statistics,
        statistics::health_statistics,
    )
)]
pub struct ApiDoc;

/// All routes. With `docs`, also the API documentation at /api/docs.
pub fn router(state: AppState) -> Router {
    router_with(state, true)
}

pub fn router_with(state: AppState, docs: bool) -> Router {
    let router = Router::new()
        .route("/api/health", get(health::health))
        .route("/api/heatmap", get(heatmap::heatmap))
        .route("/api/activities", get(activities::activities))
        .route("/api/activities/{id}", get(activity::activity))
        .route("/api/activities/{id}/file", get(file::activity_file))
        .route("/api/activities/{id}/sync", post(activity::resync_activity))
        .route(
            "/api/activities/{id}/result",
            put(results::save_result).delete(results::delete_result),
        )
        .route(
            "/api/activities/{id}/images/{image_id}",
            get(file::activity_image),
        )
        .route("/api/activity-types", get(activity_types::activity_types))
        .route(
            "/api/training-blocks",
            get(training_blocks::training_blocks),
        )
        .route(
            "/api/routes",
            get(routes::list_routes).post(routes::create_route),
        )
        .route(
            "/api/routes/import",
            post(routes::import_route)
                .layer(axum::extract::DefaultBodyLimit::max(routes::MAX_GPX_BYTES)),
        )
        .route(
            "/api/routes/{id}",
            get(routes::route)
                .put(routes::update_route)
                .delete(routes::delete_route),
        )
        .route("/api/routes/{id}/file", get(routes::route_file))
        .route("/api/export", get(archive::export))
        .route(
            "/api/import",
            post(archive::import).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/api/stats", get(stats::stats))
        .route(
            "/api/sync",
            get(sync::sync_status)
                .post(sync::start_sync)
                .delete(sync::cancel_sync),
        )
        .merge(authentication::limited(
            Router::new()
                .route("/api/auth/login", get(authentication::login))
                .route("/api/auth/callback", get(authentication::callback))
                .route("/api/auth/logout", post(authentication::logout)),
        ))
        .route(
            "/api/me",
            get(authentication::me).delete(authentication::delete_me),
        )
        .route(
            "/api/me/garmin-key",
            put(authentication::set_garmin_key).delete(authentication::delete_garmin_key),
        )
        .route(
            "/api/statistics/activities",
            get(statistics::activity_statistics),
        )
        .route("/api/statistics/health", get(statistics::health_statistics))
        .with_state(state)
        // Logs each request with its status and duration, at debug level.
        .layer(tower_http::trace::TraceLayer::new_for_http());
    if docs {
        router.merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
    } else {
        router
    }
}
