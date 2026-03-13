//! Centralized database type aliases.
//!
//! Re-exports database-specific types from [`sqlx`] under shorter names,
//! giving the rest of the crate a single place to reference them.
//!
//! Exactly one of `postgres` or `sqlite` must be enabled.

#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!("features `postgres` and `sqlite` are mutually exclusive — enable only one");

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
compile_error!("one of features `postgres` or `sqlite` must be enabled");

// ── Postgres ──────────────────────────────────────────────────────────────

#[cfg(feature = "postgres")]
pub use sqlx::PgConnection as Connection;
#[cfg(feature = "postgres")]
pub use sqlx::PgPool as Pool;
#[cfg(feature = "postgres")]
pub use sqlx::Postgres as Db;
#[cfg(feature = "postgres")]
pub use sqlx::postgres::{
    PgArgumentBuffer as ArgumentBuffer, PgRow as Row, PgTypeInfo as TypeInfo,
};

/// Fetches the current timestamp from the database via `SELECT NOW()`.
#[cfg(feature = "postgres")]
pub async fn database_now(
    executor: impl sqlx::Executor<'_, Database = Db>,
) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>("SELECT NOW()")
        .fetch_one(executor)
        .await
}

/// Extract the conflicting value from a database constraint violation, if possible.
#[cfg(feature = "postgres")]
pub fn extract_constraint_value(db_err: &dyn sqlx::error::DatabaseError) -> Option<String> {
    db_err
        .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
        .and_then(|pg_err| crate::error::parse_constraint_detail_value(pg_err.detail()))
}

// ── SQLite ────────────────────────────────────────────────────────────────

#[cfg(feature = "sqlite")]
pub use sqlx::Sqlite as Db;
#[cfg(feature = "sqlite")]
pub use sqlx::SqliteConnection as Connection;
#[cfg(feature = "sqlite")]
pub use sqlx::SqlitePool as Pool;
#[cfg(feature = "sqlite")]
pub use sqlx::sqlite::SqliteRow as Row;

/// Obtain the current database time.
///
/// SQLite does not have a native `NOW()` that returns a proper timestamp type,
/// so we fall back to `chrono::Utc::now()` on the application side.
#[cfg(feature = "sqlite")]
pub async fn database_now(
    _executor: impl sqlx::Executor<'_, Database = Db>,
) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    Ok(chrono::Utc::now())
}

/// Extract the conflicting value from a database constraint violation, if possible.
///
/// SQLite does not provide detail messages like PostgreSQL, so this always returns `None`.
#[cfg(feature = "sqlite")]
pub fn extract_constraint_value(_db_err: &dyn sqlx::error::DatabaseError) -> Option<String> {
    None
}
