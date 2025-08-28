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

pub mod context;
pub mod error;
pub mod events;
pub mod idempotent;
mod macros;
pub mod nested;
pub mod one_time_executor;
pub mod operation;
pub mod pagination;
pub mod query;
pub mod traits;

pub mod prelude {
    //! Convenience re-export of crates that the derive macros reference in generated code.

    pub use chrono;
    pub use serde;
    pub use serde_json;
    pub use sqlx;
    pub use uuid;

    #[cfg(feature = "json-schema")]
    pub use schemars;

    #[cfg(feature = "sim-time")]
    pub use sim_time;
}

#[doc(inline)]
pub use context::*;
#[doc(inline)]
pub use error::*;
pub use es_entity_macros::EsEntity;
pub use es_entity_macros::EsEvent;
pub use es_entity_macros::EsRepo;
pub use es_entity_macros::es_event_context;
pub use es_entity_macros::expand_es_query;
pub use es_entity_macros::retry_on_concurrent_modification;
#[doc(inline)]
pub use events::*;
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
pub use traits::*;

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
