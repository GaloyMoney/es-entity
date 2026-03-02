# fn update_all

The `update_all` function is a batch version of `update`.
It takes a mutable slice of entities and persists all new events in bulk.
Returns the total number of events persisted. Entities without new events are skipped.

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
#     NameUpdated { name: String },
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
#     fn try_from_events(mut events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         let mut name = String::new();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { name: n, .. } => name = n.clone(),
#                 UserEvent::NameUpdated { name: n } => name = n.clone(),
#             }
#         }
#         Ok(User { id: events.id().clone(), name, events })
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

impl User {
    pub fn change_name(&mut self, name: String) {
        self.events.push(UserEvent::NameUpdated { name: name.clone() });
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

    // First create some users
    let new_users = vec![
        NewUser { id: UserId::new(), name: "James".to_string() },
        NewUser { id: UserId::new(), name: "Roger".to_string() }
    ];
    let mut users_vec = users.create_all(new_users).await?;

    // Mutate them
    users_vec[0].change_name("Jimmy".to_string());
    users_vec[1].change_name("Rodger".to_string());

    // Persist all changes in bulk
    let n_events = users.update_all(&mut users_vec).await?;
    assert_eq!(n_events, 2);

    Ok(())
}
```
