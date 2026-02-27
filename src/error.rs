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

/// Internal error type used by `EsQuery` and `PopulateNested` when loading entities.
#[derive(Error, Debug)]
pub enum EsRepoLoadError {
    #[error("EsRepoLoadError - Sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("EsRepoLoadError - HydrationError: {0}")]
    HydrationError(#[from] EntityHydrationError),
    #[error("EsRepoLoadError - NotFound")]
    NotFound,
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
