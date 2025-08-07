# es-entity

[![Crates.io](https://img.shields.io/crates/v/es-entity)](https://crates.io/crates/es-entity)
[![Documentation](https://docs.rs/es-entity/badge.svg)](https://docs.rs/es-entity)
[![Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

An Event Sourcing Entity Framework for Rust that simplifies building event-sourced applications with PostgreSQL. 

The framework enables writing Entities that are:
- **Event Sourced** - Entities are hydrated via event projection.
- **Idempotent** - Built-in guards against duplicate operations
- **Testable** - Clean separation between domain logic and persistence

Persisted to postgres with:
- **Minimal boilerplate** - Derive macros generate repository methods automatically
- **Compile-time verified** - All SQL queries are checked at compile time via [sqlx](https://github.com/launchbadge/sqlx)
- **Optimistic concurrency** - Automatic detection of concurrent updates via event sequences
- **Pagination** - Cursor-based pagination out of the box

[Book](https://galoymoney.github.io/es-entity/index.html) |
[API Docs](https://docs.rs/es-entity/latest/es_entity/) |
[GitHub repository](https://github.com/GaloyMoney/es-entity) |
[Cargo package](https://crates.io/crates/es-entity)


## Quick Example

### Entity
First you need your entity:

```rust
// Define your entity ID (can be any type fulfilling the traits).
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
// derive_builder::Builder is optional but useful for hydrating
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,
    // Container for your events
    events: EntityEvents<UserEvent>,
}

impl User {
    // Mutations append events
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
        let name = new_name.into();
        // Check whether the event was already recorded
        idempotency_guard!(
            self.events.iter().rev(),
            // Return Idempotent::Ignored if this pattern hits
            UserEvent::NameUpdated { name: existing_name } if existing_name == &name,
            // Stop searching here
            => UserEvent::NameUpdated { .. }
        );
        self.name = name.clone();
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}

// TryFromEvents hydrates the user entity from persisted events.
impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
        // Using derive_builder::Builder to project the current state
        // while iterating over the persisted events
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
```

### Persistence

Setup your database - each entity needs 2 tables.

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
Repository methods are generated:
```rust
// Define your repository - all CRUD operations are generated!
#[derive(EsRepo)]
#[es_repo(entity = "User", columns(name(ty = "String")))]
pub struct Users {
    pool: PgPool,
}

// // Generated Repository fns:
// impl Users {
//     // Create operations
//     async fn create(&self, new: NewUser) -> Result<User, EsRepoError>;
//     async fn create_all(&self, new: Vec<NewUser>) -> Result<Vec<User>, EsRepoError>;
//     
//     // Query operations
//     async fn find_by_id(&self, id: UserId) -> Result<User, EsRepoError>;
//     async fn find_by_name(&self, name: &str) -> Result<User, EsRepoError>;
//     
//     // Update operations
//     async fn update(&self, entity: &mut User) -> Result<(), EsRepoError>;
// 
//     // Paginated listing
//     async fn list_by_id(&self, args: PaginatedQueryArgs, direction: ListDirection) -> PaginatedQueryRet;
//
//     // etc
// }
```

### Usage
```rust
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
sqlx = "0.8.3" # Needs to be in scope for entity_id! macro
serde = { version = "1.0.219", features = ["derive"] } # To serialize the `EntityEvent`
derive_builder = "0.20.1" # For hydrating and building the entity state (optional)
```

## Advanced features
### Transactions

All Repository functions exist in 2 flavours.
The `_in_op` postfix receives an additional argument for the DB connection.
This enables atomic operations across multiple entities.

```rust
let mut tx = pool.begin().await?;
users.create_in_op(&mut tx, new_user).await?;
accounts.create_in_op(&mut tx, new_account).await?;
tx.commit().await?;
```
### Nested Entities

Support for aggregates and child entities:

```rust
#[derive(EsEntity)]
pub struct Order {
    pub id: OrderId,

    // Child entity - auto implements Parent<OrderItem> for Order
    #[es_entity(nested)]
    items: Nested<OrderItem>,

    events: EntityEvents<OrderEvent>,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    // Child repo marks the parent foreign key
    columns(order_id(ty = "OrderId", update(persist = false), parent))
)]
struct OrderItems {
    pool: PgPool,
}

#[derive(EsRepo)]
#[es_repo(
    entity = "Order",
)]
pub struct Orders {
    pool: PgPool,

    // Parent repo owns the child repo
    #[es_repo(nested)]
    items: OrderItems,
}
```

## Testing

The entity style is easily testable. Hydrate from events, mutate, assert.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    fn test_user(id: UserId) -> User {
        let events = EntityEvents::init(
            id,
            [UserEvent::Initialized { 
                id,
                name: "Alice".to_string() 
            }],
        );
        
        User::try_from_events(events).unwrap();
    }

    #[test]
    fn test_user_update() {
        let mut user = test_user(UserId::new());
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
