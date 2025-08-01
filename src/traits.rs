use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

use super::{error::EsEntityError, events::EntityEvents, nested::*};

pub trait EsEvent: DeserializeOwned + Serialize + Send + Sync {
    type EntityId: Clone
        + PartialEq
        + sqlx::Type<sqlx::Postgres>
        + Eq
        + std::hash::Hash
        + Send
        + Sync;
}

/// Required trait for converting new entities into their initial events before persistence.
///
/// All `NewEntity` types must implement this trait and its `into_events` method to emit the initial
/// events that need to be persisted. The trait is used by `create` fn to persist events
/// then used to hydrate the actual `Entity` from the event stream using `try_from_events`.
///
/// This enables the event sourcing pattern where entities are reconstructed from their
/// complete event history rather than storing current state directly.
///
/// # Example
///
/// ```rust
/// use es_entity::*;
/// use serde::{Serialize, Deserialize};
///
/// entity_id!{ UserId }
///
/// #[derive(EsEvent, Serialize, Deserialize)]
/// #[serde(tag = "type", rename_all = "snake_case")]
/// #[es_event(id = "UserId")]
/// pub enum UserEvent {
///     Initialized { id: UserId, name: String },
///     NameUpdated { name: String }
/// }
///
/// // The main 'Entity' type
/// #[derive(EsEntity)]
/// pub struct User {
///     pub id: UserId,
///     name: String,
///     events: EntityEvents<UserEvent>
/// }
/// // The 'NewEntity' type used for initialization.
/// pub struct NewUser {
///     id: UserId,
///     name: String
/// }
///
/// // The IntoEvents implementation which emits an event stream.
/// impl IntoEvents<UserEvent> for NewUser {
///     fn into_events(self) -> EntityEvents<UserEvent> {
///         EntityEvents::init(
///             self.id,
///             [UserEvent::Initialized {
///                 id: self.id,
///                 name: self.name,
///             }],
///         )
///     }
/// }
///
/// // The TryFromEvents implementation to hydrate entities by replaying events chronologically.
/// impl TryFromEvents<UserEvent> for User {
///     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
///         let mut name = String::new();
///         for event in events.iter_all() {
///              match event {
///                 UserEvent::Initialized { name: n, .. } => name = n.clone(),
///                 UserEvent::NameUpdated { name: n, .. } => name = n.clone(),
///                 // ...similarly other events can be matched
///             }
///         }
///         Ok(User { id: events.id().clone(), name, events })
///     }
/// }
/// ```
pub trait IntoEvents<E: EsEvent> {
    fn into_events(self) -> EntityEvents<E>;
}

/// Required trait for re-constructing entities from their events in chronological order.
///
/// All `Entity` types must implement this trait and its `try_from_events` method to hydrate
/// entities post-persistence, enabling event-sourcing pattern where entities are built from
/// their state mutation histories.
///
/// # Example
/// [See comprehensive usage examples][crate::IntoEvents]
pub trait TryFromEvents<E: EsEvent> {
    fn try_from_events(events: EntityEvents<E>) -> Result<Self, EsEntityError>
    where
        Self: Sized;
}

pub trait EsEntity: TryFromEvents<Self::Event> {
    type Event: EsEvent;
    type New: IntoEvents<Self::Event>;

    fn events(&self) -> &EntityEvents<Self::Event>;
    fn last_persisted(&self, n: usize) -> crate::events::LastPersisted<Self::Event> {
        self.events().last_persisted(n)
    }

    fn events_mut(&mut self) -> &mut EntityEvents<Self::Event>;
}

pub trait Parent<T: EsEntity> {
    fn nested(&self) -> &Nested<T>;
    fn nested_mut(&mut self) -> &mut Nested<T>;
}

pub trait EsRepo {
    type Entity: EsEntity;
    type Err: From<EsEntityError>;
}

#[async_trait]
pub trait PopulateNested<C>: EsRepo {
    async fn populate(
        &self,
        lookup: std::collections::HashMap<C, &mut Nested<<Self as EsRepo>::Entity>>,
    ) -> Result<(), <Self as EsRepo>::Err>;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}
