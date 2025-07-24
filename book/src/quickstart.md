# Quickstart

In this section we will get up and running in a quick-and-dirty way.
More detailed explanations will follow.

Let's assume there is a `User` entity in your domain that you wish to persist using `EsEntity`.

The first thing you will need is 2 tables in postgres.
These are referred to as the 'index table' and the 'events table'.

By convention they look like this:

```bash
$ cargo sqlx migrate add users
$ cat migrations/*_users.sql
```
```sql
-- The 'index' table that holds the latest values of some selected attributes.
CREATE TABLE users (
  -- Mandatory id column
  id UUID PRIMARY KEY,
  -- Mandatory created_at column
  created_at TIMESTAMPTZ NOT NULL,

  -- Any other columns you want a quick 'index-based' lookup
  name VARCHAR UNIQUE NULL
);

-- The table that actually stores the events sequenced per entity.
-- This table has the same columns for every entity you create
-- by convention named `<entity>_events`.
CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
```

To persist the entity we need to setup a pattern with 5 components:
- The `EntityId`
- The `EntityEvent`
- The `NewEntity`
- The `Entity` itself
- And finally the `Repository` that encodes the mapping.

Here's a complete working example:
```toml
[dependencies]
es-entity = "0.6.10"
sqlx = "0.8.3" # Needs to be in scope for entity_id! macro
serde = { version = "1.0.219", features = ["derive"] } # To serialize the `EntityEvent`
derive_builder = "0.20.1" # For hydrating and building the entity state (optional)
```

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate tokio;
# extern crate anyhow;
# extern crate derive_builder;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

// Will create a uuid::Uuid wrapper type. 
// But any type can act as the ID that fulfills:
//   Clone + PartialEq + Eq + std::hash::Hash + Send + Sync
//         + sqlx::Type<sqlx::Postgres>
es_entity::entity_id!{ UserId }

// The `EsEvent` must have `serde(tag = "type")` annotation.
#[derive(EsEvent, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
// Tell the macro what the `id` type is
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

// The `EsEntity` - using derive_builder is optional
// but useful for hydrating in the `TryFromEvents` trait.
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,

    // The `events` container - mandatory field.
    // Basically its a `Vec` wrapper with some ES specific augmentation.
    events: EntityEvents<UserEvent>,
}

impl User {
    // Mutation to update the name of a user.
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        // The idempotency_guard macro is a helper to return quickly
        // if a mutation has already been applied.
        // It is not mandatory but very useful in the context of distributed / multi-thread
        // systems to protect against replays.
        idempotency_guard!(
            self.events.iter_all().rev(),
            // If this pattern matches return Idempotent::Ignored
            UserEvent::NameUpdated { name } if name == &new_name,
            // Stop searching here
            => UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

// Any EsEntity must implement `TryFromEvents`.
// This trait is what hydrates entities after loading the events from the database
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

// The `NewEntity` - this represents the data of an entity in a pre-persisted state.
// Using derive_builder is not mandatory - any type can be used for the `NewEntity` state.
#[derive(Debug, Builder)]
pub struct NewUser {
    #[builder(setter(into))]
    pub id: UserId,
    #[builder(setter(into))]
    pub name: String,
}

impl NewUser {
    pub fn builder() -> NewUserBuilder {
        NewUserBuilder::default()
    }
}

// The `NewEntity` type must implement `IntoEvents` to get the initial events that require persisting.
impl IntoEvents<UserEvent> for NewUser {
    fn into_events(self) -> EntityEvents<UserEvent> {
        EntityEvents::init(
            self.id,
            [UserEvent::Initialized {
                id: self.id,
                name: self.name,
            }],
        )
    }
}

// The `EsRepo` that will host all the persistence operations.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "User",
    // Configure the columns that need populating in the index table
    columns(
        // The 'name' column
        name(
            // The rust type of the name attribute
            ty = "String"
)))]
pub struct Users {
    // Mandatory field so that the Repository can begin transactions
    pool: sqlx::PgPool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to postgres
    let pg_con = format!("postgres://user:password@localhost:5432/pg");
    let pool = sqlx::PgPool::connect(&pg_con).await?;

    let users = Users { pool };

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    // The returned type is the hydrated Entity
    let mut user = users.create(new_user).await?;
    
    // Using the Idempotency::did_execute() to check if we need a DB roundtrip
    if user.update_name("Dweezil").did_execute() {
        users.update(&mut user).await?;
    }

    let loaded_user = users.find_by_id(user.id).await?;

    assert_eq!(user.name, loaded_user.name);

    Ok(())
}
```
