<div align="center">

# es-entity

**A type-safe event-sourcing entity framework for rust that simplifies building event-sourced applications with PostgreSQL**

[![Crates.io](https://img.shields.io/crates/v/es-entity)](https://crates.io/crates/es-entity)
[![Documentation](https://docs.rs/es-entity/badge.svg)](https://docs.rs/es-entity)
[![Book](https://img.shields.io/badge/book-orange)](https://galoymoney.github.io/es-entity/)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

</div>

## Features

- **Event Sourcing** - Store and construct entities from their event sequences to track mutations.
- **Type Safety** - SQL queries verified at compile time with comprehensive type checking and errors.
- **Auto Generation** - Automatic repository generation with configurable query patterns
- **Idempotency** - Built-in idempotency checks with automatic duplicate detection and safe operation handling
- **Pagination** - Efficient cursor-based pagination through large datasets with optimized query performance
- **Flexible IDs** - Support for any ID type with custom ID implementations and type-safe ID handling
- **Transactions** - Full ACID transaction support with atomic operations and rollback capabilities
- **Code Generation** - Automatic code generation for common patterns for repository methods

## Quick Example

#### Define the Entity

```rust
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use es_entity::*;

// Define the entity ID
es_entity::entity_id!{ UserId }

// Define events
#[derive(EsEvent, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

// Define the entity
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,
    events: EntityEvents<UserEvent>,
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();

        idempotency_guard!(
            self.events.iter_all().rev(),
            UserEvent::NameUpdated { name } if name == &new_name,
            => UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });
        Idempotent::Executed(())
    }
}

// Hydrate entity from events
impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
        let mut builder = UserBuilder::default();
        for event in events.iter_all() {
            match event {
                UserEvent::Initialized { id, name } => {
                    builder = builder.id(*id).name(name.clone());
                }
                UserEvent::NameUpdated { name } => {
                    builder = builder.name(name.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

// New entity for creation
#[derive(Debug, Builder)]
pub struct NewUser {
    pub id: UserId,
    pub name: String,
}

// Emit events from new entities
impl IntoEvents<UserEvent> for NewUser {
    fn into_events(self) -> EntityEvents<UserEvent> {
        EntityEvents::init(
            self.id,
            [UserEvent::Initialized { id: self.id, name: self.name }],
        )
    }
}
```

#### Define the Repository

```rust
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "User",
    columns(
        name(ty = "String")
    )
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```

#### Usage

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = sqlx::PgPool::connect("postgres://user:password@localhost:5432/db").await?;
    let users = Users { pool };

    // Create a new user
    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()?;

    let mut user = users.create(new_user).await?;

    // Update the user
    if user.update_name("Dweezil").did_execute() {
        users.update(&mut user).await?;
    }

    // Load the user
    let loaded_user = users.find_by_id(user.id).await?;
    assert_eq!(user.name, loaded_user.name);

    Ok(())
}
```

## Getting Started

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
es-entity = "0.7.3"
sqlx = { version = "0.8.3", features = ["runtime-tokio-rustls", "postgres", "macros"] }
serde = { version = "1.0.219", features = ["derive"] }
derive_builder = "0.20.1"
```

### Database Setup

Create the required tables for your entity:

```sql
-- Index table for quick lookups (required)
CREATE TABLE users (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL,
  name VARCHAR UNIQUE
);

-- Events table for event sourcing (required)
CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
```

## Feature Flags

es-entity provides several optional features that can be enabled in your `Cargo.toml`:

```toml
[dependencies]
es-entity = { version = "0.7.3", features = ["graphql", "json-schema", "sim-time"] }
```

### Available Features

- **`graphql`** - Enables GraphQL integration with `async-graphql` with compatible types

- **`json-schema`** - Enables JSON Schema generation

- **`sim-time`** - Enables simulation time support for testing and other operations

## Advanced Features

### Transactions

All repository methods support transactions:

```rust
let mut tx = pool.begin().await?;
let user = users.find_by_id_in_op(&mut tx, user_id).await?;
// ... more operations
tx.commit().await?;
```

### Compile-time Query Verification

es-entity uses `sqlx` for compile-time SQL verification:

```rust
let users = es_query!(
    "SELECT name FROM users WHERE active = $1",
    true
)
.fetch_all(&pool)
.await?;
```

### Cursor-based Pagination

Efficient pagination through large datasets:

```rust
let result = users.list_by_id(
    PaginatedQueryArgs { first: 10, after: None },
    ListDirection::Ascending
).await?;

if result.has_next_page {
    let next_page = users.list_by_id(
        PaginatedQueryArgs {
            first: 10,
            after: result.end_cursor
        },
        ListDirection::Ascending
    ).await?;
}
```

### Nested Aggregates

Support for complex domain relationships:

```rust
#[derive(EsEntity)]
pub struct Subscription {
    pub id: SubscriptionId,
    #[es_entity(nested)]
    billing_periods: Nested<BillingPeriod>,
}
```

### Highly Customizable

es-entity is highly customizable using attributes. For example, you can customize the events field name:

```rust
#[derive(EsEntity)]
pub struct Order {
    pub id: OrderId,
    pub total: f64,
    #[es_entity(events)] // Mark custom events field name if not named `events`
    order_history: EntityEvents<OrderEvent>,
}
```

## Documentation

- [API Documentation](https://docs.rs/es-entity) - Full public-api documentation
- [Book](https://galoymoney.github.io/es-entity) - In-depth guide and patterns
- [Examples](https://github.com/GaloyMoney/es-entity/tree/main/tests) - Working examples

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

es-entity is built and maintained by the [Galoy](https://galoy.io) team.
