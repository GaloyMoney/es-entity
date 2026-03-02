# fn create_all

The `create_all` function is a batch version of `create`.
It takes a `Vec<NewEntity>` and returns `Vec<Entity>`.

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
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         Ok(User { id: events.id().clone(), name: "Fred".to_string(), events })
#     }
# }
use es_entity::*;

pub struct NewUser {
    id: UserId,
    name: String
}

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
    let new_users = vec![
        NewUser { id: UserId::new(), name: "James".to_string() },
        NewUser { id: UserId::new(), name: "Roger".to_string() }
    ];
    let users = users.create_all(new_users).await?;

    Ok(())
}
```
