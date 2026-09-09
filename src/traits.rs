//! Traits to orchestrate and maintain the event-sourcing pattern.

use serde::{Serialize, de::DeserializeOwned};

use std::collections::HashMap;

use super::{db, error::EntityHydrationError, events::EntityEvents, tree_query::TreeSpec};

/// Required trait for all event enums to be compatible and recognised by es-entity.
///
/// All `EntityEvent` enums implement this trait to ensure it satisfies basic requirements for
/// es-entity compatibility. The trait ensures trait implementations and compile-time validation that required fields (like id) are present.
/// Implemented by the [`EsEvent`][es_entity_macros::EsEvent] derive macro with `#[es_event]` attribute.
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
    #[cfg(feature = "instrument")]
    type EntityId: Clone
        + PartialEq
        + sqlx::Type<db::Db>
        + Eq
        + std::hash::Hash
        + Send
        + Sync
        + std::fmt::Debug;

    #[cfg(not(feature = "instrument"))]
    type EntityId: Clone + PartialEq + sqlx::Type<db::Db> + Eq + std::hash::Hash + Send + Sync;

    fn event_context() -> bool;
    fn event_type(&self) -> &'static str;

    /// Whether this event type has any `Forgettable<T>` fields.
    ///
    /// The `#[derive(EsEvent)]` macro sets this automatically via an inherent const
    /// that shadows this default. Manual implementors can override it if needed.
    #[doc(hidden)]
    const HAS_FORGETTABLE_FIELDS: bool = false;
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
///     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
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
///     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
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
    fn try_from_events(events: EntityEvents<E>) -> Result<Self, EntityHydrationError>
    where
        Self: Sized;
}

/// Required trait for all entities to be compatible and recognised by es-entity.
///
/// All `Entity` types implement this trait to satisfy the basic requirements for
/// event sourcing. The trait ensures the entity implements traits like `IntoEvents`
/// and has the required components like `EntityEvent`, with helper methods to access the events sequence.
/// Implemented by the [`EsEntity`][es_entity_macros::EsEntity] derive macro.
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
pub trait EsEntity: TryFromEvents<Self::Event> + Send {
    type Event: EsEvent;
    type New: IntoEvents<Self::Event>;

    /// Returns an immutable reference to the entity's events
    fn events(&self) -> &EntityEvents<Self::Event>;

    /// Returns the last `n` persisted events
    fn last_persisted(&self, n: usize) -> crate::events::LastPersisted<'_, Self::Event> {
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
/// Implemented by the [`EsRepo`][es_entity_macros::EsRepo] derive macro with `#[es_repo]` attributes.
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
///
/// # `in_op_only`: making the operation mandatory
///
/// `#[es_repo(in_op_only)]` generates **only** the `_in_op` variants of every
/// repo fn. The standalone fns are exactly the ones that open (or borrow) the
/// pool on the caller's behalf, so removing them makes passing an operation —
/// an [`AtomicOperation`][crate::AtomicOperation] for writes, an
/// [`IntoOneTimeExecutor`][crate::IntoOneTimeExecutor] for reads — the only way
/// to reach the database.
///
/// With no standalone fn left to open one, the pool field becomes optional. A
/// repo that holds no pool cannot begin its own operation, which is the point:
/// the discipline is enforced by construction rather than by convention.
///
/// Calling a non-`_in_op` fn on such a repo does not compile — the method does
/// not exist (note the two tests below compile a real repo, so they need the
/// test database, like the book's examples):
///
/// ```compile_fail,E0599
/// use es_entity::*;
/// use serde::{Deserialize, Serialize};
/// # fn main() {}
/// # es_entity::entity_id! { UserId }
/// # #[derive(EsEvent, Debug, Serialize, Deserialize)]
/// # #[serde(tag = "type", rename_all = "snake_case")]
/// # #[es_event(id = "UserId")]
/// # pub enum UserEvent {
/// #     Initialized { id: UserId, name: String },
/// # }
/// # pub struct NewUser { id: UserId, name: String }
/// # impl IntoEvents<UserEvent> for NewUser {
/// #     fn into_events(self) -> EntityEvents<UserEvent> { unimplemented!() }
/// # }
/// # #[derive(EsEntity)]
/// # pub struct User {
/// #     pub id: UserId,
/// #     pub name: String,
/// #     events: EntityEvents<UserEvent>,
/// # }
/// # impl TryFromEvents<UserEvent> for User {
/// #     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
/// #         unimplemented!()
/// #     }
/// # }
/// // This repo deliberately KEEPS its pool, so that `create`, were it ever
/// // generated again, would compile: that makes the E0599 below prove the fn
/// // is absent, rather than merely that its body could not build an operation.
/// #[derive(EsRepo)]
/// #[es_repo(entity = "User", in_op_only, columns(name(ty = "String")))]
/// pub struct Users {
///     pool: es_entity::db::Pool,
/// }
///
/// async fn reaches_the_db_without_an_op(repo: &Users, new_user: NewUser) {
///     // error[E0599]: no method named `create` found — `in_op_only` leaves
///     // only `create_in_op`, which demands an operation from the caller.
///     repo.create(new_user).await.unwrap();
/// }
/// ```
///
/// The `_in_op` twin of the very same call compiles:
///
/// ```
/// use es_entity::*;
/// use serde::{Deserialize, Serialize};
/// # fn main() {}
/// # es_entity::entity_id! { UserId }
/// # #[derive(EsEvent, Debug, Serialize, Deserialize)]
/// # #[serde(tag = "type", rename_all = "snake_case")]
/// # #[es_event(id = "UserId")]
/// # pub enum UserEvent {
/// #     Initialized { id: UserId, name: String },
/// # }
/// # pub struct NewUser { id: UserId, name: String }
/// # impl IntoEvents<UserEvent> for NewUser {
/// #     fn into_events(self) -> EntityEvents<UserEvent> { unimplemented!() }
/// # }
/// # #[derive(EsEntity)]
/// # pub struct User {
/// #     pub id: UserId,
/// #     pub name: String,
/// #     events: EntityEvents<UserEvent>,
/// # }
/// # impl TryFromEvents<UserEvent> for User {
/// #     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
/// #         unimplemented!()
/// #     }
/// # }
/// #[derive(EsRepo)]
/// #[es_repo(entity = "User", in_op_only, columns(name(ty = "String")))]
/// pub struct Users {}
///
/// async fn takes_the_op_from_its_caller(
///     repo: &Users,
///     op: &mut impl AtomicOperation,
///     new_user: NewUser,
/// ) {
///     repo.create_in_op(op, new_user).await.unwrap();
/// }
/// ```
pub trait EsRepo: Send {
    type Entity: EsEntity;
    type CreateError;
    type ModifyError;
    type FindError: From<sqlx::Error> + From<EntityHydrationError> + Send;
    type QueryError: From<sqlx::Error> + From<EntityHydrationError> + Send;
    type EsQueryFlavor;

    fn nested_tree_spec() -> TreeSpec;

    fn hydrate_nested_from_rows<E>(
        rows_by_tag: &mut HashMap<i32, Vec<db::Row>>,
        tag_cursor: &mut i32,
        entities: &mut [Self::Entity],
    ) -> Result<(), E>
    where
        E: From<sqlx::Error> + From<EntityHydrationError>;
}

pub trait RetryableInto<T>: Into<T> + Copy + std::fmt::Debug {}
impl<T, O> RetryableInto<O> for T where T: Into<O> + Copy + std::fmt::Debug {}
