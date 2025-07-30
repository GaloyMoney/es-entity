# es_query

Given the query we arrived at in the previous section - this is what a `find_by_name` `fn` could look like:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# fn main () {}
# use serde::{Deserialize, Serialize};
# use es_entity::*;
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
use sqlx::PgPool;
use es_entity::*;

pub struct Users {
    pool: PgPool
}
impl Users {
    pub async fn find_by_name(&self, name: String) -> Result<User, EsRepoError> {
        let rows = sqlx::query_as!(
            GenericEvent::<UserId>,
            r#"
            WITH target_entity AS (
              SELECT id
              FROM users
              WHERE name = $1
            )
            SELECT e.id as entity_id, e.sequence, e.event, e.recorded_at
            FROM user_events e
            JOIN target_entity te ON e.id = te.id
            ORDER BY e.sequence;
        "#,
            name,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(EntityEvents::load_first(rows)?)
    }
}
```

The `es_query!` macro is a helper that only needs to know the `inner` part of the query
and adds the event selection part when it gets expanded.

Another way to write the function above is:
```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# fn main () {}
# use serde::{Deserialize, Serialize};
# use es_entity::*;
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
use sqlx::PgPool;
use es_entity::*;

// The `es_query!` macro only works within `fn`s defined on structs with `EsRepo` derived.
#[derive(EsRepo)]
#[es_repo(entity = "User")]
pub struct Users {
    pool: PgPool
}
impl Users {
    pub async fn find_by_name(&self, name: String) -> Result<User, EsRepoError> {
        let res = es_query!(&self.pool, "SELECT id FROM users WHERE name = $1", name,)
            .fetch_one()
            .await?;
        Ok(res)
    }
}
```

The  `es_query!` expands the event selection part of the query.
The `fetch_one()` `fn` intends to mimic the `sqlx` interface but will hydrate one entity (instead of returning one row).

The expansion of `es_query` results in a call to the `sqlx::query_as!` macro - which means that you still get typesafety and compile time column validation.
