//! Centralized database type aliases.
//!
//! Re-exports database-specific types from [`sqlx`] under shorter names,
//! giving the rest of the crate a single place to reference them.
//!
//! Exactly one of the `postgres` or `sqlite` features must be enabled.

#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!("features `postgres` and `sqlite` are mutually exclusive; enable only one");

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
compile_error!("one of the features `postgres` or `sqlite` must be enabled");

// ── Postgres ──────────────────────────────────────────────────────────────

#[cfg(feature = "postgres")]
pub use sqlx::PgConnection as Connection;
#[cfg(feature = "postgres")]
pub use sqlx::PgPool as Pool;
#[cfg(feature = "postgres")]
pub use sqlx::Postgres as Db;
#[cfg(feature = "postgres")]
pub use sqlx::postgres::{
    PgArgumentBuffer as ArgumentBuffer, PgArguments as Arguments, PgRow as Row,
    PgTypeInfo as TypeInfo,
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

// ── SQLite ────────────────────────────────────────────────────────────────

#[cfg(feature = "sqlite")]
pub use sqlx::Sqlite as Db;
#[cfg(feature = "sqlite")]
pub use sqlx::SqliteConnection as Connection;
#[cfg(feature = "sqlite")]
pub use sqlx::SqlitePool as Pool;
#[cfg(feature = "sqlite")]
pub use sqlx::sqlite::{
    SqliteArguments as Arguments, SqliteRow as Row, SqliteTypeInfo as TypeInfo,
};

/// Obtains the current time.
///
/// SQLite has no `NOW()` returning a timestamp type, so this is the
/// application clock rather than the database's.
#[cfg(feature = "sqlite")]
pub async fn database_now(
    _executor: impl sqlx::Executor<'_, Database = Db>,
) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    Ok(chrono::Utc::now())
}
