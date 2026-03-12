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
