# fn find_all

The `find_all` function allows you to fetch multiple entities by their IDs in a single database query.

```rust,ignore
fn find_all(&self, ids: &[EntityId]) -> Result<HashMap<EntityId, Entity>, EntityError>
fn find_all_in_op(&self, op: OP, ids: &[EntityId]) -> Result<HashMap<EntityId, Entity>, EntityError>
```

This is more efficient than calling `find_by_id` multiple times, as it performs a single database query with `WHERE id = ANY($1)`.

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
# pub struct NewUser { id: UserId, name: String }
use es_entity::*;

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    name: String,
    events: EntityEvents<UserEvent>,
}

#[derive(EsRepo)]
#[es_repo(entity = "User")]
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
    
    // Create multiple users
    let user1 = users.create(NewUser { id: UserId::new(), name: "Alice".to_string() }).await?;
    let user2 = users.create(NewUser { id: UserId::new(), name: "Bob".to_string() }).await?;
    let user3 = users.create(NewUser { id: UserId::new(), name: "Charlie".to_string() }).await?;

    // Fetch multiple users by their IDs
    let ids = vec![user1.id.clone(), user2.id.clone(), user3.id.clone()];
    let found_users = users.find_all(&ids).await?;
    
    assert_eq!(found_users.len(), 3);
    assert!(found_users.contains_key(&user1.id));
    assert!(found_users.contains_key(&user2.id));
    assert!(found_users.contains_key(&user3.id));

    Ok(())
}
```

The function returns a `HashMap` where the keys are the entity IDs and the values are the entities. This makes it easy to look up entities by their ID after fetching them in bulk.

