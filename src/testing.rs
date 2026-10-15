//! Building entities in tests without a repository.
//!
//! Enabled by the `testing` feature. Add it to `[dev-dependencies]` only, so
//! it is part of `cargo test` builds and absent from production builds:
//!
//! ```toml
//! [dev-dependencies]
//! es-entity = { version = "*", features = ["testing"] }
//! ```
//!
//! Then `use es_entity::testing::*;` at the top of a `#[cfg(test)]` module:
//!
//! ```rust
//! use es_entity::{testing::*, *};
//! use serde::{Deserialize, Serialize};
//!
//! entity_id! { UserId }
//!
//! #[derive(EsEvent, Debug, Serialize, Deserialize)]
//! #[serde(tag = "type", rename_all = "snake_case")]
//! #[es_event(id = "UserId")]
//! pub enum UserEvent {
//!     Initialized { id: UserId, name: String },
//!     NameUpdated { name: String },
//! }
//!
//! #[derive(EsEntity)]
//! pub struct User {
//!     pub id: UserId,
//!     pub name: String,
//!     events: EntityEvents<UserEvent>,
//! }
//!
//! impl TryFromEvents<UserEvent> for User {
//!     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
//!         let mut name = String::new();
//!         for event in events.iter_all() {
//!             match event {
//!                 UserEvent::Initialized { name: n, .. } | UserEvent::NameUpdated { name: n } => {
//!                     name = n.clone()
//!                 }
//!             }
//!         }
//!         Ok(User { id: events.id().clone(), name, events })
//!     }
//! }
//!
//! pub struct NewUser {
//!     name: String,
//! }
//!
//! impl IntoEvents<UserEvent> for NewUser {
//!     type Id = Generated;
//!
//!     fn into_events(self, builder: EntityEventsBuilder<UserEvent, Generated>) -> EntityEvents<UserEvent> {
//!         let id = builder.id();
//!         builder.initial_events([UserEvent::Initialized { id, name: self.name }])
//!     }
//! }
//!
//! let id = UserId::new();
//! let user = User::hydrate_from_events(
//!     id,
//!     [
//!         UserEvent::Initialized { id, name: "Alice".to_string() },
//!         UserEvent::NameUpdated { name: "Bob".to_string() },
//!     ],
//! )
//! .unwrap();
//! assert_eq!(user.name, "Bob");
//!
//! let user = User::hydrate_from_new(UserId::new(), NewUser { name: "Alice".to_string() }).unwrap();
//! assert_eq!(user.name, "Alice");
//! assert!(!user.events().any_new());
//! ```
//!
//! An entity built this way is what a repository load returns: its events
//! are all persisted, recorded at [`Clock::now`].

use crate::{
    clock::Clock,
    error::EntityHydrationError,
    events::{EntityEvents, hydrate_checked},
    id_policy::{EntityEventsBuilder, Generated, Supplied},
    traits::{EsEntity, EsEvent, IntoEvents},
};

fn hydrate_persisted<E: EsEntity>(
    mut events: EntityEvents<E::Event>,
) -> Result<E, EntityHydrationError> {
    events.mark_new_events_persisted_at(Clock::now());
    hydrate_checked(events.widen_snapshot())
}

/// Rebuilds an entity from an arbitrary event stream.
pub trait HydrateFromEvents: EsEntity + Sized {
    /// The entity whose events are `events`, stored under `id`.
    fn hydrate_from_events(
        id: <Self::Event as EsEvent>::EntityId,
        events: impl IntoIterator<Item = Self::Event>,
    ) -> Result<Self, EntityHydrationError> {
        hydrate_persisted(EntityEvents::init(id, events))
    }
}

impl<T: EsEntity> HydrateFromEvents for T {}

/// Builds an entity from its `New*` type when the repository generates the
/// id: the test picks it.
pub trait HydrateFromNewGenerated: EsEntity + Sized {
    /// The entity `new` creates under `id`.
    fn hydrate_from_new(
        id: <Self::Event as EsEvent>::EntityId,
        new: Self::New,
    ) -> Result<Self, EntityHydrationError>;
}

impl<T> HydrateFromNewGenerated for T
where
    T: EsEntity,
    T::New: IntoEvents<T::Event, Id = Generated>,
{
    fn hydrate_from_new(
        id: <Self::Event as EsEvent>::EntityId,
        new: Self::New,
    ) -> Result<Self, EntityHydrationError> {
        hydrate_persisted(new.into_events(EntityEventsBuilder::__generated(id)))
    }
}

/// Builds an entity from its `New*` type when the `New*` type brings its
/// own id.
pub trait HydrateFromNewSupplied: EsEntity + Sized {
    /// The entity `new` creates.
    fn hydrate_from_new(new: Self::New) -> Result<Self, EntityHydrationError>;
}

impl<T> HydrateFromNewSupplied for T
where
    T: EsEntity,
    T::New: IntoEvents<T::Event, Id = Supplied>,
{
    fn hydrate_from_new(new: Self::New) -> Result<Self, EntityHydrationError> {
        hydrate_persisted(new.into_events(EntityEventsBuilder::__supplied()))
    }
}
