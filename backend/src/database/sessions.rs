use anyhow::Result;
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;

use super::Pool;
use crate::database::schema::{sessions, users};
use crate::models::User;

/// Also removes expired sessions of all accounts, so that they do not pile up.
pub async fn create_session(
    pool: &Pool,
    user_id: i64,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let mut connection = pool.get().await?;
    diesel::delete(sessions::table.filter(sessions::expires_at.le(diesel::dsl::now)))
        .execute(&mut connection)
        .await?;
    diesel::insert_into(sessions::table)
        .values((
            sessions::token_hash.eq(token_hash),
            sessions::user_id.eq(user_id),
            sessions::expires_at.eq(expires_at),
        ))
        .execute(&mut connection)
        .await?;
    Ok(())
}

/// The account of a session that has not expired.
pub async fn session_user(pool: &Pool, token_hash: &[u8]) -> Result<Option<User>> {
    Ok(sessions::table
        .inner_join(users::table)
        .filter(sessions::token_hash.eq(token_hash))
        .filter(sessions::expires_at.gt(diesel::dsl::now))
        .select(User::as_select())
        .first(&mut pool.get().await?)
        .await
        .optional()?)
}

/// Also removes expired sessions of all accounts.
pub async fn delete_session(pool: &Pool, token_hash: &[u8]) -> Result<()> {
    diesel::delete(
        sessions::table.filter(
            sessions::token_hash
                .eq(token_hash)
                .or(sessions::expires_at.le(diesel::dsl::now)),
        ),
    )
    .execute(&mut pool.get().await?)
    .await?;
    Ok(())
}
