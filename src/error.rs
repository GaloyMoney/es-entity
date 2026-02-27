//! Types for working with errors produced by es-entity.

use thiserror::Error;

/// Error type for entity hydration failures (reconstructing entities from events).
///
/// Previously named `EsEntityError`. Now only contains hydration-related concerns.
#[derive(Error, Debug)]
pub enum EntityHydrationError {
    #[error("EntityHydrationError - UninitializedFieldError: {0}")]
    UninitializedFieldError(#[from] derive_builder::UninitializedFieldError),
    #[error("EntityHydrationError - Deserialization: {0}")]
    EventDeserialization(#[from] serde_json::Error),
}

/// Deprecated alias for `EntityHydrationError`.
#[deprecated(note = "renamed to EntityHydrationError")]
pub type EsEntityError = EntityHydrationError;

/// Trait for error types that can represent a "not found" condition.
///
/// Implemented by error types used with `EsQuery::fetch_one`, which needs
/// to produce a "not found" error when no rows match.
pub trait FromNotFound {
    fn not_found() -> Self;
}

/// Internal error type used by `persist_events` for event table unique violations.
#[derive(Error, Debug)]
pub enum EsRepoPersistError {
    #[error("EsRepoPersistError - Sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("EsRepoPersistError - ConcurrentModification")]
    ConcurrentModification,
}

#[derive(Error, Debug)]
#[error("CursorDestructureError: couldn't turn {0} into {1}")]
pub struct CursorDestructureError(&'static str, &'static str);

impl From<(&'static str, &'static str)> for CursorDestructureError {
    fn from((name, variant): (&'static str, &'static str)) -> Self {
        Self(name, variant)
    }
}
