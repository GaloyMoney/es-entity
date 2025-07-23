# Quickstart

In this section we will get up and running in a quick-and-dirty way.
More detailed explanations will follow.

## Complete Example

Let's assume there is a `User` entity in your domain that you wish to persist using `EsEntity`.

The first thing you will need is 2 tables in postgres.
These are referred to as the 'index table' and the 'events table'.

By convention they look like this:

```bash
$ cargo sqlx migrate add users
```

cat migrations/*_users.sql
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

-- The table that actually stores the events sequenced per entity
-- This table has the same columns for every entity you create (by convention named `<entity>_events`).
CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
```

To persist the entity we need to setup a pattern with 5 parts:
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
//
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
}

// The `EsEntity` - using derive_builder is optional
// but useful for projecting in the `TryFromEvents` trait.
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,

    // The `events` container - mandatory field.
    // Basically its a `Vec` wrapper with some ES specific augmentation.
    events: EntityEvents<UserEvent>,
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
            }
        }
        builder.events(events).build()
    }
}

// The `New` entity - this represents the data of an entity in a pre-persisted state.
// Using derive_builder is not mandatory - any type can be used for the `New` state.
#[derive(Debug, Builder)]
pub struct NewUser {
    #[builder(setter(into))]
    pub id: UserId,
    #[builder(setter(into))]
    pub name: String,
}

// The `New` type must implement `IntoEvents` to get the initial events that require persisting.
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("This is a library example - use the async functions in your application");
    Ok(())
}
```
