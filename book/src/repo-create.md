# fn create

The `create` fn takes a `NewEntity` type and returns an `Entity` type.
Internally it `INSERT`s a row into the `index` table and persists all the events returned from the `IntoEvents::into_events` `fn` in the `events` table.

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
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         EntityEvents::init(
#             self.id,
#             [UserEvent::Initialized {
#                 id: self.id,
#                 name: self.name,
#             }],
#         )
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         Ok(User { id: events.id().clone(), name: "Fred".to_string(), events })
#     }
# }
use es_entity::*;

pub struct NewUser {
    id: UserId,
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
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let users = Users { pool: init_pool().await? };
    let new_user = NewUser { id: UserId::new(), name: "Fred".to_string() };
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
    ) -> Result<User, es_entity::EsRepoError> {
        let id = &new_entity.id;
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
The `create(accessor = "<>")` option modifies how the field is accessed.

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
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         unimplemented!()
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
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

pub struct NewUser { id: UserId, some_hidden_field: String }
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
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         unimplemented!()
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
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
pub struct NewUser { id: UserId }

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
