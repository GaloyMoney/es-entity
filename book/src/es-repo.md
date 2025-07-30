# Es Repo

Deriving the `EsRepo` macro on a struct will generate a bunch of CRUD `fns` (and some additional supporting structs) to interact with the persistence layer.

For this to work the `Entity` you intend to persist / load must be setup as described in the `Entity` section (with an `Event`, `Id`, `NewEntity` and `Entity`) type.

As a minimum you must specify the `entity` attribute and have a field that holds a `PgPool` type.

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
# pub struct NewUser { id: UserId, name: String }
# impl IntoEvents<UserEvent> for NewUser {
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         unimplemented!()
#     }
# }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     events: EntityEvents<UserEvent>,
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         unimplemented!()
#     }
# }
use es_entity::*;

#[derive(EsRepo)]
#[es_repo(
    entity = "User",

    // defaults that get derived if not explicitly configured:
    // id = "UserId",                  // The type of the `id`
    // new = "NewUser",                // The type of the `NewEntity`
    // event = "UserEvent",            // The type of the `Event` enum
    // err = "EsRepoError",            // The Error type that should be returned from all fns.
    // tbl = "users",                  // The name of the index table
    // events_tbl = "user_events",     // The name of the events table
    // tbl_prefix = "",                // A table prefix that should be added to the derived table names

    // Columns specify a list of attributes that get mapped to the index table:
    // columns(
    //     id(ty = "UserId", list_by)  // The id column is always mapped - no need to specify it
    // )
)]
pub struct Users {
    pool: sqlx::PgPool

    // Marker if you use a name other than `pool`.
    // #[es_entity(pool)]
    // different_name_for_pool: sqlx::PgPool
}
```

But there are a number of additional options that cate be passed to `es_repo` to modify the behaviour or type of functions it generates.

The most important of which is the `columns` option that configures the mapping from entity attributes to index table columns.

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

pub struct NewUser { id: UserId, name: String }

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    name: String,
    events: EntityEvents<UserEvent>,
}

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        // Declares that there is a `name` column on the `index` table.
        // The rust type for it is `String`.
        // Without further configuration `EsRepo` will assume both the `NewEntity`
        // and the `Entity` types have an accessible `.name` attribute
        // for populating and updating the index table.
        name = "String",
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

Take a look at the next sections to see more information on how the options modify the generated code.
