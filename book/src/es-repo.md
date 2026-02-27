# EsRepo

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
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         unimplemented!()
#     }
# }
use es_entity::*;

#[derive(EsRepo)]
#[es_repo(
    entity = "User",

    // Defaults that get derived if not explicitly configured:
    // id = "UserId",                  // The type of the `id`
    // new = "NewUser",                // The type of the `NewEntity`
    // event = "UserEvent",            // The type of the `Event` enum
    // Per-operation error types are generated: UserCreateError, UserModifyError, UserFindError, UserQueryError
    // tbl = "users",                  // The name of the index table
    // events_tbl = "user_events",     // The name of the events table
    // tbl_prefix = "",                // A table prefix that should be added to the derived table names

    // Columns specify a list of attributes that get mapped to the index table:
    // columns(
    //     The id column is always mapped - no need to specify it
    //     id(ty = "UserId", list_by)
    // )
)]
pub struct Users {
    pool: sqlx::PgPool

    // Marker if you use a name other than `pool`.
    // #[es_entity(pool)]
    // different_name_for_pool: sqlx::PgPool
}
```

There are a number of options that can be passed to `es_repo` to modify the behaviour or type of functions it generates.

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
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
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
        
        // The above is equivalent to the more explicit notation:
        // name(ty = "String")
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
```

### Column options

Each column supports the following options:

| Option | Description |
|--------|-------------|
| `ty = "Type"` | **(required)** The Rust type of the column |
| `create(accessor = "...")` | Custom accessor on `NewEntity` for insert (see [create](./repo-create.md)) |
| `create(persist = false)` | Skip this column during insert |
| `update(accessor = "...")` | Custom accessor on `Entity` for update (see [update](./repo-update.md)) |
| `update(persist = false)` | Skip this column during update |
| `list_by` | Generate `list_by_<column>` pagination query |
| `list_for` | Include in `list_for_<column>` filtering |
| `constraint = "name"` | Map a custom DB constraint name to this column for error reporting (see [Error Types](./repo-errors.md)) |

Take a look at the next sections to see more information on how the options modify the generated code.
