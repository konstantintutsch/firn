use anyhow::Result;
use diesel::dsl::exists;
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde_json::Value;
use uuid::Uuid;

use super::Pool;
use crate::database::schema::{activities, routes};
use crate::models::{NewRoute, RouteRecord, RouteSummary};

/// The IDs of all of the account's routes.
pub async fn route_ids(pool: &Pool, user_id: i64) -> Result<Vec<Uuid>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .select(routes::id)
        .load(&mut pool.get().await?)
        .await?)
}

/// Deletes some of the account's routes.
pub async fn delete_routes(pool: &Pool, user_id: i64, ids: &[Uuid]) -> Result<()> {
    diesel::delete(
        routes::table
            .filter(routes::user_id.eq(user_id))
            .filter(routes::id.eq_any(ids)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

pub async fn insert_route(pool: &Pool, route: &NewRoute<'_>) -> Result<RouteSummary> {
    Ok(diesel::insert_into(routes::table)
        .values(route)
        .returning(RouteSummary::as_select())
        .get_result(&mut pool.get().await?)
        .await?)
}

/// The account's routes, newest first.
pub async fn list_routes(pool: &Pool, user_id: i64) -> Result<Vec<RouteSummary>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .order((routes::created_at.desc(), routes::id.desc()))
        .select(RouteSummary::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

/// One route of the account, with its points.
pub async fn route(pool: &Pool, user_id: i64, id: Uuid) -> Result<Option<(RouteSummary, Value)>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .filter(routes::id.eq(id))
        .select((RouteSummary::as_select(), routes::points))
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// Renames a route of the account. False if it has no such route.
pub async fn update_route(
    pool: &Pool,
    user_id: i64,
    id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<bool> {
    let changed = diesel::update(
        routes::table
            .filter(routes::user_id.eq(user_id))
            .filter(routes::id.eq(id)),
    )
    .set((routes::name.eq(name), routes::description.eq(description)))
    .execute(&mut pool.get().await?)
    .await?;
    Ok(changed > 0)
}

/// Removes a route of the account.
pub async fn delete_route(pool: &Pool, user_id: i64, id: Uuid) -> Result<()> {
    diesel::delete(
        routes::table
            .filter(routes::user_id.eq(user_id))
            .filter(routes::id.eq(id)),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}

/// All of the account's routes, oldest first.
pub async fn route_records(pool: &Pool, user_id: i64) -> Result<Vec<RouteRecord>> {
    Ok(routes::table
        .filter(routes::user_id.eq(user_id))
        .order((routes::created_at.asc(), routes::id.asc()))
        .select(RouteRecord::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

/// Saves a route from an export under its own ID, replacing an earlier import of
/// it. False if another account has a route with this ID. The route keeps its
/// activity only if the account has that activity.
pub async fn import_route(
    pool: &Pool,
    user_id: i64,
    route: &RouteRecord,
    preview: &Value,
) -> Result<bool> {
    let mut connection = pool.get().await?;
    let owner: Option<i64> = routes::table
        .find(route.id)
        .select(routes::user_id)
        .first(&mut connection)
        .await
        .optional()?;
    if owner.is_some_and(|owner| owner != user_id) {
        return Ok(false);
    }
    let activity_id = match route.activity_id {
        Some(id) => diesel::select(exists(
            activities::table
                .find(id)
                .filter(activities::user_id.eq(user_id)),
        ))
        .get_result::<bool>(&mut connection)
        .await?
        .then_some(id),
        None => None,
    };
    let row = (
        routes::id.eq(route.id),
        routes::user_id.eq(user_id),
        routes::name.eq(&route.name),
        routes::description.eq(&route.description),
        routes::distance_m.eq(route.distance_m),
        routes::elevation_gain_m.eq(route.elevation_gain_m),
        routes::points.eq(&route.points),
        routes::preview.eq(preview),
        routes::activity_id.eq(activity_id),
        routes::created_at.eq(route.created_at),
    );
    diesel::insert_into(routes::table)
        .values(row)
        .on_conflict(routes::id)
        .do_update()
        .set(row)
        .execute(&mut connection)
        .await?;
    Ok(true)
}
