/// Prevent duplicate event processing by checking for idempotent operations.
///
/// Guards against replaying the same mutation in event-sourced systems.
/// Returns `Idempotent::Ignored` early if matching events are found, allowing the caller
/// to skip redundant operations. Use break pattern to allow re-applying past operations.
///
/// # Parameters
///
/// - `$events`: Event collection to search (usually chronologically reversed)
/// - `$pattern`: Event patterns that indicate operation already applied
/// - `$break_pattern`: Optional break pattern to stop searching
///
/// # Examples
///
/// ```rust
/// // Basic: prevent duplicate operations
/// idempotency_guard!(
///     self.events.iter().rev(),
///     UserEvent::NameUpdated { name } if name == &new_name
/// );
///
/// // With break: allow re-applying past operations
/// idempotency_guard!(
///     self.events.iter().rev(),
///     UserEvent::NameUpdated { name } if name == &new_name,
///     => UserEvent::NameUpdated { .. }  // Stop at any name update
/// );
/// ```
#[macro_export]
macro_rules! idempotency_guard {
    ($events:expr, $( $pattern:pat $(if $guard:expr)? ),+ $(,)?) => {
        for event in $events {
            match event {
                $(
                    $pattern $(if $guard)? => return $crate::FromIdempotentIgnored::from_ignored(),
                )+
                _ => {}
            }
        }
    };
    ($events:expr, $( $pattern:pat $(if $guard:expr)? ),+,
     => $break_pattern:pat $(if $break_guard:expr)?) => {
        for event in $events {
            match event {
                $($pattern $(if $guard)? => return $crate::FromIdempotentIgnored::from_ignored(),)+
                $break_pattern $(if $break_guard)? => break,
                _ => {}
            }
        }
    };
}

/// Execute an event-sourced query with automatic entity hydration.
///
/// Executes user-defined queries and returns entities by internally
/// joining with events table and hydrating entities, essentially giving the
/// illusion of working with just the index table.
///
/// # Parameters
///
/// - `tbl_prefix`: Table prefix to ignore when deriving entity names from table names (optional)
/// - `entity`: Override the entity type (optional, useful when table name doesn't match entity name)
/// - SQL query string
/// - Additional arguments for the SQL query (optional)
///
/// # Examples
/// ```ignore
/// // Basic usage
/// es_query!("SELECT id FROM users WHERE id = $1", id)
///
/// // With table prefix
/// es_query!(
///     tbl_prefix = "app",
///     "SELECT id FROM app_users WHERE active = true"
/// )
///
/// // With custom entity type
/// es_query!(
///     entity = User,
///     "SELECT id FROM custom_users_table WHERE id = $1",
///     id as UserId
/// )
/// ```
#[macro_export]
macro_rules! es_query {
    // With entity override
    (
        entity = $entity:ident,
        $query:expr,
        $($args:tt)*
    ) => ({
        $crate::expand_es_query!(
            entity = $entity,
            sql = $query,
            args = [$($args)*]
        )
    });
    // With entity override - no args
    (
        entity = $entity:ident,
        $query:expr
    ) => ({
        $crate::expand_es_query!(
            entity = $entity,
            sql = $query
        )
    });

    // With tbl_prefix
    (
        tbl_prefix = $tbl_prefix:literal,
        $query:expr,
        $($args:tt)*
    ) => ({
        $crate::expand_es_query!(
            tbl_prefix = $tbl_prefix,
            sql = $query,
            args = [$($args)*]
        )
    });
    // With tbl_prefix - no args
    (
        tbl_prefix = $tbl_prefix:literal,
        $query:expr
    ) => ({
        $crate::expand_es_query!(
            tbl_prefix = $tbl_prefix,
            sql = $query
        )
    });

    // Basic form
    (
        $query:expr,
        $($args:tt)*
    ) => ({
        $crate::expand_es_query!(
            sql = $query,
            args = [$($args)*]
        )
    });
    // Basic form - no args
    (
        $query:expr
    ) => ({
        $crate::expand_es_query!(
            sql = $query
        )
    });
}

/// Implement error handling for types that wrap `EsEntityError`.
///
/// Adds `From<EsEntityError>` conversion and utility methods for checking specific error types.
/// Required for integration with `EsRepo` trait and event-sourced operations.
///
/// # Requirements
///
/// Your error type must have an `EsEntityError` variant and sqlx::Error variant for sqlx operations.
/// ```rust
/// #[derive(Error, Debug)]
/// pub enum MyError {
///     #[error("Database error: {0}")]
///     Database(#[from] sqlx::Error), // ← Required variant
///     #[error("{0}")]
///     EsEntityError(EsEntityError), // ← Required variant
/// }
/// ```
///
/// # Generated Methods
///
/// - `was_not_found()` - checks for `NotFound` errors
/// - `was_concurrent_modification()` - checks for `ConcurrentModification` errors
///
/// `EsEntityError` includes: `NotFound`, `ConcurrentModification`, `UninitializedFieldError`, `EventDeserialization`
///
/// # Examples
///
/// ```rust
/// from_es_entity_error!(MyError);
///
/// // Now works with EsRepo
/// impl EsRepo for MyRepository {
///     type Err = MyError;  // ← Implements From<EsEntityError>
/// }
///
/// // Check error types
/// if error.was_not_found() {
///     // Handle not found
/// }
/// ```
#[macro_export]
macro_rules! from_es_entity_error {
    ($name:ident) => {
        impl $name {
            pub fn was_not_found(&self) -> bool {
                matches!(self, $name::EsEntityError($crate::EsEntityError::NotFound))
            }
            pub fn was_concurrent_modification(&self) -> bool {
                matches!(
                    self,
                    $name::EsEntityError($crate::EsEntityError::ConcurrentModification)
                )
            }
        }
        impl From<$crate::EsEntityError> for $name {
            fn from(e: $crate::EsEntityError) -> Self {
                $name::EsEntityError(e)
            }
        }
    };
}

// Helper macro for common entity_id implementations (internal use only)
#[doc(hidden)]
#[macro_export]
macro_rules! __entity_id_common_impls {
    ($name:ident) => {
        impl $name {
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                $crate::prelude::uuid::Uuid::new_v4().into()
            }
        }

        impl From<$crate::prelude::uuid::Uuid> for $name {
            fn from(uuid: $crate::prelude::uuid::Uuid) -> Self {
                Self(uuid)
            }
        }

        impl From<$name> for $crate::prelude::uuid::Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl From<&$name> for $crate::prelude::uuid::Uuid {
            fn from(id: &$name) -> Self {
                id.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::prelude::uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self($crate::prelude::uuid::Uuid::parse_str(s)?))
            }
        }
    };
}

// Helper macro for GraphQL-specific entity_id implementations (internal use only)
#[doc(hidden)]
#[macro_export]
macro_rules! __entity_id_graphql_impls {
    ($name:ident) => {
        impl From<$crate::graphql::UUID> for $name {
            fn from(id: $crate::graphql::UUID) -> Self {
                $name($crate::prelude::uuid::Uuid::from(&id))
            }
        }

        impl From<&$crate::graphql::UUID> for $name {
            fn from(id: &$crate::graphql::UUID) -> Self {
                $name($crate::prelude::uuid::Uuid::from(id))
            }
        }
    };
}

// Helper macro for additional conversions (internal use only)
#[doc(hidden)]
#[macro_export]
macro_rules! __entity_id_conversions {
    ($($from:ty => $to:ty),* $(,)?) => {
        $(
            impl From<$from> for $to {
                fn from(id: $from) -> Self {
                    <$to>::from($crate::prelude::uuid::Uuid::from(id))
                }
            }
            impl From<$to> for $from {
                fn from(id: $to) -> Self {
                    <$from>::from($crate::prelude::uuid::Uuid::from(id))
                }
            }
        )*
    };
}

/// Create UUID-wrappers for database operations.
///
/// This macro generates type-safe UUID-wrapper structs with trait support for
/// serialization, database operations, GraphQL integration, and JSON schema generation.
///
/// # Features
///
/// The macro automatically includes different trait implementations based on enabled features:
/// - `graphql`: Adds GraphQL UUID conversion traits
/// - `json-schema`: Adds JSON schema generation support
///
/// # Generated Traits
///
/// All entity IDs automatically implement:
/// - `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`
/// - `serde::Serialize`, `serde::Deserialize` (with transparent serialization)
/// - `sqlx::Type` (with transparent database type)
/// - `Display` and `FromStr` for string conversion
/// - `From<Uuid>` and `From<EntityId>` for UUID conversion
///
/// # Parameters
///
/// - `$name`: One or more entity ID type names to create
/// - `$from => $to`: Optional conversion pairs between different entity ID types
///
/// # Examples
///
/// ```rust
/// use es_entity::entity_id;
///
/// entity_id!(UserId, OrderId);
///
/// // Creates:
/// // pub struct UserId(Uuid);
/// // pub struct OrderId(Uuid);
/// ```
///
/// ```rust
/// use es_entity::entity_id;
///
/// entity_id!(
///     UserId,
///     AdminUserId;
///     UserId => AdminUserId
/// );
///
/// // Creates UserId and AdminUserId with automatic conversion between them
/// ```
#[macro_export]
macro_rules! entity_id {
    // Match identifiers without conversions
    ($($name:ident),+ $(,)?) => {
        $crate::entity_id! { $($name),+ ; }
    };
    ($($name:ident),+ $(,)? ; $($from:ty => $to:ty),* $(,)?) => {
        $(
            #[cfg_attr(feature = "json-schema", derive($crate::prelude::schemars::JsonSchema))]
            #[derive(
                $crate::prelude::sqlx::Type,
                Debug,
                Clone,
                Copy,
                PartialEq,
                Eq,
                PartialOrd,
                Ord,
                Hash,
                $crate::prelude::serde::Deserialize,
                $crate::prelude::serde::Serialize,
            )]
            #[serde(transparent)]
            #[sqlx(transparent)]
            pub struct $name($crate::prelude::uuid::Uuid);

            $crate::__entity_id_common_impls!($name);

            #[cfg(feature = "graphql")]
            $crate::__entity_id_graphql_impls!($name);
        )+
        $crate::__entity_id_conversions!($($from => $to),*);
    };
}
