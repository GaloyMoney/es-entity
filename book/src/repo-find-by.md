# fn find_by

Every `column` that gets configured on the `EsRepo` will get the following `fn`s:

```rust,ignore
fn find_by_<column> -> Result<Entity, EntityError>
fn maybe_find_by_<column> -> Result<Option<Entity>, EntityError>
```

It is assumed that your database schema has a relevant `INDEX` on `<column>` to make the lookup efficient.

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
# pub struct NewUser { id: UserId, name: String }
use es_entity::*;

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
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
    users.create(new_user).await?;

    let user = users.find_by_name("Fred").await?;
    assert_eq!(user.name, "Fred");

    let user = users.maybe_find_by_name("No Body").await?;
    assert!(user.is_none());

    Ok(())
}
```
