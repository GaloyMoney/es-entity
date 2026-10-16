# fn create

The `create` fn takes a `NewEntity` type and returns an `Entity` type.
It `INSERT`s a row into the `index` table and persists all the events returned from the `IntoEvents::into_events` `fn` in the `events` table.

We must use the `columns` option to specify which columns need inserting into the `index` table

In the code below we want to include a `name` column in the `index` table that requires mapping.
```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate tokio;
# extern crate anyhow;
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
# }
# impl IntoEvents<UserEvent> for NewUser {
#     type IdSource = FromRepo;
#     fn into_events(self, builder: EntityEventsBuilder<UserEvent, FromRepo>) -> EntityEvents<UserEvent> {
#         let id = builder.id();
#         builder.initial_events([UserEvent::Initialized {
#             id,
#             name: self.name,
#         }])
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         Ok(User { id: events.id().clone(), name: "Fred".to_string(), events })
#     }
# }
use es_entity::*;

pub struct NewUser {
    // The `name` attribute on the `NewEntity` must be accessible
    // for inserting into the `index` table.
    name: String
}

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    // The name attribute on the `Entity` must be accessible 
    // for updates of the `index` table.
    name: String,
    events: EntityEvents<UserEvent>,
}

#[derive(EsRepo)]
#[es_repo(entity = "User", columns(name = "String"))]
pub struct Users {
    pool: sqlx::PgPool
}

# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = std::env::var("PG_CON").unwrap_or_else(|_| "postgres://user:password@localhost:5432/pg".to_string());
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let users = Users { pool: init_pool().await? };
    let new_user = NewUser { name: "Fred".to_string() };
    // The `create` fn takes a `NewEntity` and returns a persisted and hydrated `Entity`
    let _user = users.create(new_user).await?;

    Ok(())
}
```

The insert part of the `create` function looks somewhat equivalent to:
```rust,ignore
impl Users {
    pub async fn create(
        &self,
        new_entity: NewUser
    ) -> Result<User, es_entity::RepoFault> {
        // The repository mints the id
        let id = &UserId::new();
        // The attribute specified in the `columns` option
        let name = &new_entity.name;

        sqlx::query!("INSERT INTO users (id, name) VALUES ($1, $2)",
            id as &UserId,
            name as &String
        )
        .execute(self.pool())
        .await?;

        // persist events
        // hydrate entity
        // execute post_persist_hook
        // return entity
    }
}
```
The key thing to configure is how the columns of the index table get populated via the `create` option.
The `create(accessor = "<>")` option modifies how the field is accessed on the `NewEntity` type.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# fn main () {}
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
# }
# impl IntoEvents<UserEvent> for NewUser {
#     type IdSource = FromRepo;
#     fn into_events(self, _builder: EntityEventsBuilder<UserEvent, FromRepo>) -> EntityEvents<UserEvent> {
#         unimplemented!()
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         unimplemented!()
#     }
# }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     name: String,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

pub struct NewUser { some_hidden_field: String }
impl NewUser {
    fn my_name(&self) -> String {
        self.some_hidden_field.clone()
    }
}

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        // Instead of using the `name` field on the `NewEntity` struct
        // the generated code will use: `new_entity.my_name()`
        // to populate the `name` column.
        name(ty = "String", create(accessor = "my_name()")),
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

The `create(persist = false)` option omits inserting the column during creation.
This is useful for dynamic values that don't become known until later on in the entities lifecycle.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# fn main () {}
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
# }
# impl IntoEvents<UserEvent> for NewUser {
#     type IdSource = FromRepo;
#     fn into_events(self, _builder: EntityEventsBuilder<UserEvent, FromRepo>) -> EntityEvents<UserEvent> {
#         unimplemented!()
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         unimplemented!()
#     }
# }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     name: String,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

// There is no `name` attribute because we do not initially insert into this column.
pub struct NewUser;

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        name(ty = "String", create(persist = false)),
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

## create_or_find_in_op

For entities that may already exist, the repo also gets one
`create_or_find_in_op`: it creates the entity, or returns the stored one when
the `NewEntity`'s unique keys already name it.

```rust,ignore
pub enum Ensured<E> { Created(E), Found(E) }

pub async fn create_or_find_in_op<OP>(&self, op: &mut OP, new: NewAccount)
    -> Result<Ensured<Account>, es_entity::RepoWriteError<AccountEnsureRejection>>
```

`Ensured::into_inner` returns the entity however it came about, `is_created`
says which it was.

### The four outcomes

- **`Created`**: the insert hit no unique constraint. Events are persisted and
  the post-persist hook runs, as in `create_in_op`.
- **`Found`**: the insert hit a unique constraint, and the `new` entity's unique
  keys (id included) match exactly one stored entity. That entity is returned
  unchanged: `new`'s events are dropped, nothing is written, the hook does not
  run. The stored entity's id may differ from `new`'s id (it was found through
  another key). Callers take the id from the returned entity, never from the
  `new` they passed in.
- **`Rejected(KeysSpanEntities)`**: the `new` entity's keys match more than one
  stored entity (its id is held by one, its code by another). No single stored
  entity is the one the caller described. This rejection does not abort the
  caller's transaction: the insert was `DO NOTHING` and the lookup is a
  `SELECT`.
- **`Transient(OptimisticConflict)`**: the insert conflicted and the lookup
  found nothing. Not reachable under `READ COMMITTED` (the lookup runs as a
  second statement and sees the committed row); kept for `REPEATABLE READ` /
  `SERIALIZABLE`, where the insert itself usually raises 40001 first
  (`Transient(SerializationFailure)`).

`Found` compares nothing but keys: a `new` whose non-key fields differ from the
stored entity still gets the stored entity. Ensure never updates.

> **Take the id from the returned entity.** `Found` can return an entity whose
> id is not the one in the `NewEntity` you passed in. Reading the id off your
> `new` after the call is a bug. A caller that wants "found by id is fine, a
> foreign code is an error" compares the returned entity's id with the id it
> minted and rejects itself, or uses `create_in_op`.

### The statements

In the caller's operation, one statement inserts the index row with
`ON CONFLICT DO NOTHING` (no conflict target) and, from the row it returned,
the events:

```sql
WITH new_row AS (
  INSERT INTO <table> (<columns>) VALUES (..)
  ON CONFLICT DO NOTHING
  RETURNING id
)
INSERT INTO <events_table> (..) SELECT .. FROM new_row .. RETURNING ..
```

Without a target Postgres arbitrates on every unique constraint, unique index,
partial unique index and exclusion constraint. A conflicting row that another
transaction has not committed yet blocks the insert until that transaction
finishes. If it rolled back, the insert proceeds, so there is no retry loop to
write. An empty result is the conflict.

Only then a second statement, with a fresh snapshot, looks the stored entity up
by every unique key at once:

```sql
SELECT id, .. FROM <table>
WHERE id = $1
   OR code = $2
   OR (account = $3 AND currency = $4)
```

One disjunct per key, a composite key an `AND` group. Every value is a bind
parameter taken from the `NewEntity` the way `create` reads it,
`create(accessor = ..)` included. An optional key that is `None` is bound as
`NULL`, and `col = NULL` never matches: an absent optional key does not take
part. One matching row is loaded and returned as `Found`; two or more are
`KeysSpanEntities`; none is `Transient`.

Under `READ COMMITTED` the lookup sees the row the other transaction committed.
Under `REPEATABLE READ` or `SERIALIZABLE` the insert cannot see that row and
Postgres raises a serialization failure instead, which is a `Transient`
(`SerializationFailure`) fault to retry in a new transaction. A lookup that
finds nothing is `Transient(OptimisticConflict)`. Neither is ever
`CorruptState`.

### The keys

The id is always a key. Besides it, a key is every unique constraint, or
non-partial unique index, that the migrations declare for the table, on columns
the create writes as-is (not `create(persist = false)`, not `forgettable`).

A table the migrations do not describe has no schema catalog. The method is
still generated, with the id as its only key and `RepoFault` as its error: the
insert needs no catalog to arbitrate, and ensuring by id is always sound. No
other column is assumed unique.

A repo whose only key is the id cannot have its keys span entities, so it
returns `Result<Ensured<E>, RepoFault>`; no rejection type is generated for it.

No method is generated for a scoped repo (the lookup needs a scope the create
has no way to name) or for a soft-deleting repo (a soft-deleted row still holds
its key, so the insert conflicts on it, but the lookup filters deleted rows and
finds nothing: every ensure of a deleted key would be `Transient`).

A conflict the lookup cannot explain also ends in `Transient(OptimisticConflict)`:
a partial unique index, an expression index, or a unique column that is
`create(persist = false)` or `forgettable` still arbitrates the insert, but is
not a lookup key, because the catalog has no predicate and the create does not
write the value as-is. If its columns are not also part of a full unique key,
the lookup finds nothing.

### The rejection type

`<Entity>EnsureRejection` has the member `KeysSpanEntities { ids, keys }`: the
distinct stored ids the lookup returned and the names of the keys that matched,
joined with `_and_` for a composite key. It carries ids and column names, never
key values. Its code is `<table>_keys_span_entities`.

It also has the repo's check-constraint members, with the same codes as the
constraint violation, and every nested child's rejections, since a check
violation or a child's create can still reject the insert. A unique violation
never reaches it: the insert is `DO NOTHING`.
