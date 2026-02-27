# fn list_by

To load a whole page of entities at once you can set the `list_by` option on the column.
This will generate the `list_by_<column>` `fn`s and appropriate `cursor` structs.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate tokio;
# extern crate anyhow;
# extern crate uuid;
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
#     Deleted
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
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     name: String,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    // list_by will generate the UsersByNameCursor
    columns(name(ty = "String", list_by))
)]
pub struct Users {
    pool: sqlx::PgPool
}

// // Generated code:
// pub mod user_cursor {
//    pub struct UsersByNameCursor {
//        name: String
//        // id is always added to disambiguate
//        // incase the `name` column is not unique
//        id: UserId,
//    }
//
//    // Cursors that always exist:
//    pub struct UsersById {
//        id: UserId,
//    }
//
//    pub struct UsersByCreatedAt {
//        created_at: chrono::DateTime<chrono::Utc>
//        id: UserId,
//    }
// }
//
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let users = Users { pool: init_pool().await? };
    let new_user = NewUser { id: UserId::new(), name: "Fred".to_string() };
    users.create(new_user).await?;

    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_by_id(
            PaginatedQueryArgs {
                first: 5,
                // after: None represents beginning of the list
                after: Some(user_cursor::UsersByIdCursor {
                    id: uuid::Uuid::nil().into(),
                }),
            },
            ListDirection::Ascending,
        )
        .await?;
    assert!(!entities.is_empty());

    // To collect all entities in a loop you can use `into_next_query()`.
    // This is not recommended - just to highlight the API.
    let mut query = Default::default();
    let mut all_users = Vec::new();
    loop {
        let mut res = users.list_by_name(query, Default::default()).await?;
        all_users.extend(res.entities.drain(..));
        if let Some(next_query) = res.into_next_query() {
            query = next_query;
        } else {
            break;
        }
    }
    assert!(!all_users.is_empty());

    Ok(())
}
```
