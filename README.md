# es-entity

[![Crates.io](https://img.shields.io/crates/v/es-entity)](https://crates.io/crates/es-entity)
[![Documentation](https://docs.rs/es-entity/badge.svg)](https://docs.rs/es-entity)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A **type-safe** Event Sourcing Entity Framework for Rust that simplifies building event-sourced applications with PostgreSQL. 

## Features at a glance

- 🛡️ **Type-safe** - All SQL queries are checked at compile time via [sqlx]
- 🏗️ **Minimal boilerplate** - Derive macros generate repository methods automatically
- 🔄 **Event sourcing patterns** - Built-in support for events, entities, and aggregates
- 🔒 **Optimistic concurrency** - Automatic handling via event sequences
- 🎯 **Idempotency** - Built-in guards against duplicate operations
- 📄 **Pagination** - Cursor-based pagination out of the box
- 🔗 **GraphQL ready** - Optional integration with [async-graphql]
- 🧪 **Testable** - Clean separation between domain logic and persistence

## Quick Example

```rust
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

// Define your entity ID
es_entity::entity_id! { UserId }

// Define your events
#[derive(EsEvent, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

// Define your entity
#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    pub name: String,
    events: EntityEvents<UserEvent>,
}

// Define your repository - all CRUD operations are generated!
#[derive(EsRepo)]
#[es_repo(entity = "User", columns(name(ty = "String")))]
pub struct Users {
    pool: PgPool,
}

// Use it!
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPool::connect("postgres://localhost/myapp").await?;
    let users = Users { pool };
    
    // Create a new user
    let user = users.create(NewUser {
        id: UserId::new(),
        name: "Alice".to_string(),
    }).await?;
    
    // Query by indexed columns
    let alice = users.find_by_name("Alice").await?;
    
    // Update with automatic idempotency
    let mut user = users.find_by_id(user.id).await?;
    if user.update_name("Alice Cooper").did_execute() {
        users.update(&mut user).await?;
    }
    
    Ok(())
}
```

## Getting Started

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
es-entity = "0.7"
sqlx = { version = "0.8", features = ["postgres", "uuid", "chrono", "json"] }
serde = { version = "1.0", features = ["derive"] }
```

### Database Setup

Each entity requires two tables:

```sql
-- Index table for queries
CREATE TABLE users (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL,
  name VARCHAR UNIQUE  -- Add columns you want to query by
);

-- Event storage table
CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
```

### Core Concepts

#### 1. **Entity ID**
A strongly-typed identifier for your entities:
```rust
es_entity::entity_id! { UserId }
// Or use your own type that implements required traits
```

#### 2. **Events**
Events represent state changes and must be serializable:
```rust
#[derive(EsEvent, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}
```

#### 3. **Entity**
Your domain model that is built from events:
```rust
#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    pub name: String,
    events: EntityEvents<UserEvent>,  // Required field
}

impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
        // Rebuild state from events
    }
}
```

#### 4. **Repository**
Handles all persistence operations:
```rust
#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(name(ty = "String", unique))  // Define indexed columns
)]
pub struct Users {
    pool: PgPool,
}
```

## Generated Repository Methods

The `EsRepo` derive macro generates a complete set of type-safe repository methods:

```rust
impl Users {
    // Create operations
    async fn create(&self, new: NewUser) -> Result<User, EsRepoError>;
    async fn create_all(&self, new: Vec<NewUser>) -> Result<Vec<User>, EsRepoError>;
    
    // Query operations
    async fn find_by_id(&self, id: UserId) -> Result<User, EsRepoError>;
    async fn find_by_name(&self, name: &str) -> Result<User, EsRepoError>;
    
    // Update operations
    async fn update(&self, entity: &mut User) -> Result<(), EsRepoError>;

    // etc
}
```

## Advanced Features

### Idempotency

Protect against duplicate operations:

```rust
impl User {
    pub fn update_name(&mut self, new_name: String) -> Idempotent<()> {
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
```

### Nested Entities

Support for aggregates and child entities:

```rust
#[derive(EsEntity)]
pub struct Order {
    pub id: OrderId,

    #[es_entity(nested)]
    items: Nested<OrderItem>,

    events: EntityEvents<OrderEvent>,
}

// Child repo marks the parent foreign key
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    columns(order_id(ty = "OrderId", update(persist = false), parent))
)]
pub struct OrderItems {
    pool: PgPool,
}

// Parent repo owns the child repo
#[derive(EsRepo)]
#[es_repo(
    entity = "Order",
)]
pub struct Orders {
    pool: PgPool,

    #[es_repo(nested)]
    items: OrderItems,
}
```

### Transactions

Atomic operations across multiple entities:

```rust
let mut tx = pool.begin().await?;
users.create_in_op(&mut tx, new_user).await?;
accounts.create_in_op(&mut tx, new_account).await?;
tx.commit().await?;
```

## Testing

The entity style is easily testable. Hydrate from events, mutate, assert.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_user_update() {
        let events = EntityEvents::init(
            UserId::new(),
            [UserEvent::Initialized { 
                id: UserId::new(), 
                name: "Alice".to_string() 
            }],
        );
        
        let mut user = User::try_from_events(events).unwrap();
        assert_eq!(user.update_name("Bob"), Idempotent::Executed(()));
        assert_eq!(user.update_name("Bob"), Idempotent::Ignored(()));
    }
}
```

## Documentation

- [API Documentation](https://docs.rs/es-entity)
- [Book](https://galoymoney.github.io/es-entity) - In-depth guide and patterns

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
