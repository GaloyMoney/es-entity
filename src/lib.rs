//! A Rust library for persisting Event Sourced entities to PostgreSQL
//!
//! This crate simplifies Event Sourcing persistence by automatically generating type-safe
//! queries and operations for PostgreSQL. It decouples domain logic from persistence
//! concerns while ensuring compile-time query verification via [sqlx](https://crates.io/crates/sqlx).
//!
//! # Documentation
//! - [Book](https://galoymoney.github.io/es-entity)
//! - [Github repository](https://github.com/GaloyMoney/es-entity)
//! - [Cargo package](https://crates.io/crates/es-entity)
//!
//! # Features
//!
//! - Store and construct from event sequences
//! - Type-safe and compile-time verification
//! - Simple and configurable query generation
//! - Easy idempotency checks
//! - Cursor-based pagination
//! - Flexible ID types
//! - Atomic operations

#![cfg_attr(feature = "fail-on-warnings", deny(warnings))]
#![cfg_attr(feature = "fail-on-warnings", deny(clippy::all))]
#![forbid(unsafe_code)]

pub mod clock;
mod constraint;
pub mod context;
pub mod db;
pub mod error;
pub use constraint::*;
pub mod events;
pub mod forgettable;
pub mod idempotent;
mod macros;
pub mod nested;
pub mod one_time_executor;
pub mod operation;
pub mod pagination;
pub mod query;
pub mod snapshot;
pub mod sql_commenter;
pub mod traits;
pub mod tree_query;

pub mod prelude {
    //! Convenience re-export of crates that the derive macros reference in generated code.

    pub use chrono;
    pub use serde;
    pub use serde_json;
    pub use sqlx;
    pub use tokio;
    pub use uuid;

    #[cfg(feature = "json-schema")]
    pub use schemars;
}

#[doc(inline)]
pub use context::*;
pub use errlanes;
pub use errlanes::{
    Denied, Fail, Failure, Fatal, FatalKind, Fault, Lane, Laned, Lift, Liftable, Rejection,
    Settled, Transient, TransientKind,
};
#[doc(inline)]
pub use error::*;
#[doc(hidden)]
pub use es_entity_macros::ConstraintRejection;
pub use es_entity_macros::EsEntity;
pub use es_entity_macros::EsEvent;
pub use es_entity_macros::EsRepo;
pub use es_entity_macros::EsSnapshot;
pub use es_entity_macros::es_event_context;
pub use es_entity_macros::expand_es_query;
#[doc(inline)]
pub use events::*;
#[doc(inline)]
pub use forgettable::{Forgettable, ForgettableRef};
#[doc(inline)]
pub use idempotent::*;
#[doc(inline)]
pub use nested::*;
#[doc(inline)]
pub use one_time_executor::*;
#[doc(inline)]
pub use operation::*;
#[doc(inline)]
pub use pagination::*;
#[doc(inline)]
pub use query::*;
#[doc(inline)]
pub use snapshot::*;
#[doc(inline)]
pub use traits::*;
#[doc(inline)]
pub use tree_query::*;

#[cfg(feature = "graphql")]
pub mod graphql {
    pub use async_graphql;
    pub use base64;

    #[derive(Debug, serde::Serialize, serde::Deserialize, Clone, Copy)]
    #[serde(transparent)]
    pub struct UUID(crate::prelude::uuid::Uuid);
    async_graphql::scalar!(UUID);
    impl<T: Into<crate::prelude::uuid::Uuid>> From<T> for UUID {
        fn from(id: T) -> Self {
            let uuid = id.into();
            Self(uuid)
        }
    }
    impl From<&UUID> for crate::prelude::uuid::Uuid {
        fn from(id: &UUID) -> Self {
            id.0
        }
    }
}
