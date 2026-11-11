use anyhow::Result;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::Value;

use super::Pool;
use crate::schema::{activities, users};

#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = users)]
pub struct User {
    pub id: i64,
    pub email: Option<String>,
    pub name: Option<String>,
    pub garmin_api_key: Option<Vec<u8>>,
}

/// Finds or creates the account of an OpenID Connect identity. The first account
/// claims the activities that were saved before accounts existed.
pub async fn sign_in_user(
    pool: &Pool,
    issuer: &str,
    subject: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<(User, bool)> {
    let mut connection = pool.get().await?;
    let result = connection
        .transaction::<_, diesel::result::Error, _>(async |connection| {
            // Serializes first sign-ins, so that only one account can be first.
            diesel::sql_query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
                .execute(&mut *connection)
                .await?;
            let existing: Option<User> = users::table
                .filter(users::oidc_issuer.eq(issuer))
                .filter(users::oidc_subject.eq(subject))
                .select(User::as_select())
                .first(&mut *connection)
                .await
                .optional()?;
            if let Some(user) = existing {
                let user = diesel::update(users::table.find(user.id))
                    .set((users::email.eq(email), users::name.eq(name)))
                    .returning(User::as_returning())
                    .get_result(&mut *connection)
                    .await?;
                return Ok((user, false));
            }
            let is_first = users::table
                .count()
                .get_result::<i64>(&mut *connection)
                .await?
                == 0;
            let user = diesel::insert_into(users::table)
                .values((
                    users::oidc_issuer.eq(issuer),
                    users::oidc_subject.eq(subject),
                    users::email.eq(email),
                    users::name.eq(name),
                ))
                .returning(User::as_returning())
                .get_result(&mut *connection)
                .await?;
            if is_first {
                diesel::update(activities::table.filter(activities::user_id.is_null()))
                    .set(activities::user_id.eq(user.id))
                    .execute(&mut *connection)
                    .await?;
            }
            Ok((user, is_first))
        })
        .await?;
    Ok(result)
}

/// The status of the account's last finished sync.
pub async fn last_sync(pool: &Pool, user_id: i64) -> Result<Option<Value>> {
    Ok(users::table
        .find(user_id)
        .select(users::last_sync)
        .first(&mut pool.get().await?)
        .await?)
}

pub async fn save_last_sync(pool: &Pool, user_id: i64, status: &Value) -> Result<()> {
    diesel::update(users::table.find(user_id))
        .set(users::last_sync.eq(status))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// Deletes an account. Its activities, health data and sessions go with it.
pub async fn delete_user(pool: &Pool, user_id: i64) -> Result<()> {
    diesel::delete(users::table.find(user_id))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}

/// The accounts that have a Garmin key, for the automatic sync.
pub async fn users_with_keys(pool: &Pool) -> Result<Vec<User>> {
    Ok(users::table
        .filter(users::garmin_api_key.is_not_null())
        .select(User::as_select())
        .load(&mut pool.get().await?)
        .await?)
}

pub async fn set_garmin_key(pool: &Pool, user_id: i64, sealed: Option<&[u8]>) -> Result<()> {
    diesel::update(users::table.find(user_id))
        .set(users::garmin_api_key.eq(sealed))
        .execute(&mut pool.get().await?)
        .await?;
    Ok(())
}
