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
# extern crate tokio;
# extern crate anyhow;
es_entity::entity_id!{ UserId }         // Will create a uuid::Uuid wrapper type. 

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("This is a library example - use the async functions in your application");
    Ok(())
}
```
