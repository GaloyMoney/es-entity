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

## create_or_find_by_&lt;key&gt;_in_op

For entities that may already exist, the repo also gets
`create_or_find_by_<key>_in_op`: it creates the entity, or returns the stored
one when a row with the same key is already there.

```rust,ignore
pub enum Ensured<E> { Created(E), Found(E) }

pub async fn create_or_find_by_code_in_op<OP>(&self, op: &mut OP, new: NewTemplate)
    -> Result<Ensured<Template>, es_entity::RepoWriteError<TemplateEnsureByCodeRejection>>
```

`Ensured::into_inner` returns the entity however it came about, `is_created`
says which it was.

### Which keys

- `create_or_find_by_id_in_op` on a repo with `id(source = caller)` (see
  [id](./repo-id.md)).
- `create_or_find_by_<columns>_in_op` for every unique constraint, or
  non-partial unique index, that the migrations declare for the table, on
  columns the create writes as-is. The name joins the columns with `_and_`:
  a unique `(account_id, currency)` is
  `create_or_find_by_account_id_and_currency_in_op`. The key's value is read
  from the `NewEntity` the way `create` reads it, `create(accessor = ..)`
  included.

No method is generated for a column with `create(persist = false)` or a
`forgettable` column, for a partial unique index (the conflict target would
need its `WHERE` predicate), for an expression index, for a repo whose table
the migrations do not describe (no key is assumed unique), for a scoped repo
(the lookup needs a scope the create has no way to name), or for a soft-deleting
repo (a soft-deleted row still holds its key but is invisible to the lookup).

### What it does

In the caller's operation, one statement inserts the index row with
`ON CONFLICT (<key columns>) DO NOTHING` and, from the row it returned, the
events:

1. A conflicting row that another transaction has not committed yet blocks the
   insert until that transaction finishes. If it rolled back, the insert
   proceeds, so there is no retry loop to write.
2. A row was inserted: the post-persist hook runs, nested children are created
   and the result is `Ensured::Created(entity)`.
3. No row was inserted: the stored entity is looked up by the key, inside the
   operation, and the result is `Ensured::Found(entity)`. **The `NewEntity`'s
   events are dropped, nothing is written, and the post-persist hook does not
   run.** `Found` means "this already existed", so what the caller passed in
   is only the description of the key.
4. Any other unique or check violation is the rejection.

Under `READ COMMITTED` step 3 sees the row the other transaction committed.
Under `REPEATABLE READ` or `SERIALIZABLE` the statement cannot see that row and
Postgres raises a serialization failure instead, which is a `Transient`
(`SerializationFailure`) fault to retry in a new transaction. A lookup that
finds nothing is `Transient(OptimisticConflict)`. Neither is ever
`CorruptState`.

### The rejection type

The rejection is the repo's constraint violation without the member the method
turns into `Found`: `<Entity>EnsureBy<Key>Rejection`, for example
`TemplateEnsureByCodeRejection` or `UserEnsureByIdRejection`. It has the other
members, with the same codes, so a `match` over it does not mention the
impossible case. The caller-chosen primary key stays in the rejection of a
`create_or_find_by_<key>_in_op` on a caller-sourced repo. When nothing is left
to reject on, the method returns `Result<Ensured<E>, RepoFault>`.

The rejection's code and message name the constraint, never the key's value,
and so does the `Fatal` raised for a key that cannot conflict.
