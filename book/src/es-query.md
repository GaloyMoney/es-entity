# es_query

The `es_query!` macro is a helper that allows you only to query the `index` table without needing to join with the `events` table.

The expansion of `es_query!` results in a call to the `sqlx::query_as!` macro - which means that you still get typesafety and compile time column validation.

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
            SELECT e.id as entity_id, e.sequence, e.event, e.context as "context: ContextData", e.recorded_at, NULL::jsonb as "forgettable_payload?"
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

The `es_query!` macro removes the boilerplate of fetching the events and lets you just write the part that queries the `index` table:
```sql
SELECT id FROM users WHERE name = $1
```

On expansion it constructs the complete query (adding the `JOIN` with the `events` table) and hydrates the entities from the events.
This simplifies the above implementation into:
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

#[derive(EsRepo)]
#[es_repo(entity = "User")]
pub struct Users {
    pool: PgPool
}
impl Users {
    pub async fn find_by_name(&self, name: String) -> Result<User, EsRepoError> {
        es_query!(
            "SELECT id FROM users WHERE name = $1",
            name
        ).fetch_one(&self.pool).await
    }
}
```

The `es_query!` macro only works within `fn`s defined on structs with `EsRepo` derived.

The functions intend to mimic the `sqlx` interface but instead of returning rows they return fully hydrated entities:

```rust,ignore
async fn fetch_one(<executor>) -> Result<Entity, Repo::Err>
async fn fetch_optional(<executor) -> Result<Option<Entity>, Repo::Err>

// The `(_, bool)` signifies whether or not the query could have fetched more or the list is exhausted:
async fn fetch_n(<executor>, n) -> Result<(Vec<Entity>, bool), Repo::Err>
```
