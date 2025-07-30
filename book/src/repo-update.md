# fn update

The `update` fn takes a mutable reference to an `Entity` and persists any new events that have been added to it.
It will also `UPDATE` the row in the `index` table with the latest values derived from the entities attributes.
It returns the number of events that were persisted.

In the code below we have a `name` column in the `index` table that needs to be kept in sync with the entity's state.
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
#     NameChanged { name: String },
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
#     fn try_from_events(mut events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         let mut name = String::new();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { name: n, .. } => name = n.clone(),
#                 UserEvent::NameChanged { name: n } => name = n.clone(),
#             }
#         }
#         Ok(User { id: events.id().clone(), name, events })
#     }
# }
# pub struct NewUser {
#     id: UserId,
#     name: String
# }
use es_entity::*;

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    // The name attribute on the `Entity` must be accessible 
    // for updates of the `index` table.
    name: String,
    events: EntityEvents<UserEvent>,
}

impl User {
    pub fn change_name(&mut self, name: String) {
        self.events.push(UserEvent::NameChanged { name: name.clone() });
        self.name = name;
    }
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
    
    // First create a user
    let new_user = NewUser { id: UserId::new(), name: "Fred".to_string() };
    let mut user = users.create(new_user).await?;
    
    // Now update the user
    user.change_name("Frederick".to_string());
    
    // The `update` fn takes a mutable reference to an `Entity` and persists new events
    let n_events = users.update(&mut user).await?;
    assert_eq!(n_events, 1); // One NameChanged event was persisted

    Ok(())
}
```

The update part of the `update` function looks somewhat equivalent to:
```rust,ignore
impl Users {
    pub async fn update(
        &self,
        entity: &mut User
    ) -> Result<usize, es_entity::EsRepoError> {
        // Check if there are any new events to persist
        if !entity.events().any_new() {
            return Ok(0);
        }

        let id = &entity.id;
        // The attribute specified in the `columns` option
        let name = &entity.name;

        sqlx::query!("UPDATE users SET name = $2 WHERE id = $1",
            id as &UserId,
            name as &String
        )
        .execute(self.pool())
        .await?;

        // persist new events
        // execute post_persist_hook
        // return number of events persisted
    }
}
```
The key thing to configure is how the columns of the index table get updated via the `update` option.
The `update(accessor = "<>")` option modifies how the field is accessed on the `Entity` type.

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
# pub struct NewUser { id: UserId, name: String }
use es_entity::*;

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    name: String,
    events: EntityEvents<UserEvent>,
}

impl User {
    pub fn display_name(&self) -> String {
        format!("User: {}", self.name)
    }
}

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        // Instead of using the `name` field on the `Entity` struct
        // the generated code will use: `entity.display_name()`
        // to populate the `name` column during updates.
        name(ty = "String", update(accessor = "display_name()")),
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

The `update(persist = false)` option prevents updating the column.
This is useful for columns that should never change after creation.

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
use es_entity::*;

// Assume the name of a user is immutable.
pub struct NewUser { id: UserId, name: String }

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    // Exposing the `name` attribute on the `Entity` is optional
    // as it does not need to be accessed during update.
    // name: String
    events: EntityEvents<UserEvent>,
}

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        name(ty = "String", update(persist = false))
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

Note that if no columns need updating (all columns have `update(persist = false)`), the `UPDATE` query is skipped entirely for better performance.
