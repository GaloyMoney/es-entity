//! Centralized database type aliases.
//!
//! Re-exports PostgreSQL-specific types from [`sqlx`] under shorter names,
//! giving the rest of the crate a single place to reference them.

pub use sqlx::PgConnection as Connection;
pub use sqlx::PgPool as Pool;
pub use sqlx::Postgres as Db;
pub use sqlx::postgres::{
    PgArgumentBuffer as ArgumentBuffer, PgRow as Row, PgTypeInfo as TypeInfo,
};

/// Fetches the current timestamp from the database via `SELECT NOW()`.
pub async fn database_now(
    executor: impl sqlx::Executor<'_, Database = Db>,
) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>("SELECT NOW()")
        .fetch_one(executor)
        .await
}
