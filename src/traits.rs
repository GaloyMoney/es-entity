//! Traits to orchestrate and maintain the event-sourcing pattern.

use serde::{Serialize, de::DeserializeOwned};

use super::{error::EsEntityError, events::EntityEvents, operation::AtomicOperation};

/// Required trait for all event enums to be compatible and recognised by es-entity.
///
/// All `EntityEvent` enums implement this trait to ensure it satisfies basic requirements for
/// es-entity compatibility. The trait ensures trait implementations and compile-time validation that required fields (like id) are present.
/// Implemented by the [es_entity_macros::EsEvent] derive macro with `#[es_event]` attribute.
///
/// # Example
///
/// ```compile_fail
/// use es_entity::*;
/// use serde::{Serialize, Deserialize};
///
/// entity_id!{ UserId }
///
/// // Compile-time error: missing `id` attribute in `es_event`
/// #[derive(EsEvent, Serialize, Deserialize)]
/// #[serde(tag = "type", rename_all = "snake_case")]
/// // #[es_event(id = "UserId")] <- This line is required!
/// pub enum UserEvent {
///     Initialized { id: UserId, name: String },
///     NameUpdated { name: String },
///     Deactivated { reason: String }
/// }
/// ```
///
/// Correct usage:
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
///     NameUpdated { name: String },
///     Deactivated { reason: String }
/// }
/// ```
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
/// events that need to be persisted, later the `Entity` is re-constructed by replaying these events.
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
/// // The main `Entity` type
/// #[derive(EsEntity)]
/// pub struct User {
///     pub id: UserId,
///     name: String,
///     events: EntityEvents<UserEvent>
/// }
///
/// // The `NewEntity` type used for initialization.
/// pub struct NewUser {
///     id: UserId,
///     name: String
/// }
///
/// // The `IntoEvents` implementation which emits an event stream.
/// // These events help track `Entity` state mutations
/// // Returns the `EntityEvents<UserEvent>`
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
/// // The `TryFromEvents` implementation to hydrate entities by replaying events chronologically.
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
    /// Method to implement which emits event stream from a `NewEntity`
    fn into_events(self) -> EntityEvents<E>;
}

/// Required trait for re-constructing entities from their events in chronological order.
///
/// All `Entity` types must implement this trait and its `try_from_events` method to hydrate
/// entities post-persistence.
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
///
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
/// // The `TryFromEvents` implementation to hydrate entities by replaying events chronologically.
/// // Returns the re-constructed `User` entity
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
pub trait TryFromEvents<E: EsEvent> {
    /// Method to implement which hydrates `Entity` by replaying its events chronologically
    fn try_from_events(events: EntityEvents<E>) -> Result<Self, EsEntityError>
    where
        Self: Sized;
}

/// Required trait for all entities to be compatible and recognised by es-entity.
///
/// All `Entity` types implement this trait to satisfy the basic requirements for
/// event sourcing. The trait ensures the entity implements traits like `IntoEvents`
/// and has the required components like `EntityEvent`, with helper methods to access the events sequence.
/// Implemented by the [es_entity_macros::EsEntity] derive macro.
///
/// # Example
///
/// ```compile_fail
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
/// }
///
/// // Compile-time error: Missing required trait implementations
/// // - TryFromEvents<UserEvent> for User
/// // - IntoEvents<UserEvent> for NewUser (associated type New)
/// // - NewUser type definition
/// #[derive(EsEntity)]
/// pub struct User {
///     pub id: UserId,
///     pub name: String,
///     events: EntityEvents<UserEvent>,
/// }
/// ```
pub trait EsEntity: TryFromEvents<Self::Event> {
    type Event: EsEvent;
    type New: IntoEvents<Self::Event>;

    /// Returns an immutable reference to the entity's events
    fn events(&self) -> &EntityEvents<Self::Event>;

    /// Returns the last `n` persisted events
    fn last_persisted(&self, n: usize) -> crate::events::LastPersisted<Self::Event> {
        self.events().last_persisted(n)
    }

    /// Returns mutable reference to the entity's events
    fn events_mut(&mut self) -> &mut EntityEvents<Self::Event>;
}

/// Required trait for all repositories to be compatible with es-entity and generate functions.
///
/// All repositories implement this trait to satisfy the basic requirements for
/// type-safe database operations with the associated entity. The trait ensures validation
/// that required fields (like entity) are present with compile-time errors.
/// Implemented by the [es_entity_macros::EsRepo] derive macro with `#[es_repo]` attributes.
///
/// # Example
///
/// ```ignore
///
/// // Would show error for missing entity field if not provided in the `es_repo` attribute
/// #[derive(EsRepo, Debug)]
/// #[es_repo(entity = "User", columns(name(ty = "String")))]
/// pub struct Users {
///     pool: PgPool,  // Required field for database operations
/// }
///
/// impl Users {
///     pub fn new(pool: PgPool) -> Self {
///         Self { pool }
///    }
/// }
/// ```
pub trait EsRepo {
    type Entity: EsEntity;
    type Err: From<EsEntityError> + From<sqlx::Error>;
    type EsQueryFlavor;

    /// Loads all nested entities for a given set of parent entities within an atomic operation.
    fn load_all_nested_in_op<OP>(
        op: &mut OP,
        entities: &mut [Self::Entity],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send
    where
        OP: AtomicOperation;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}
