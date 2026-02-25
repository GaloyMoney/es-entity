# fn list_for

Similar to `list_by` the `list_for` option lets you query pages of entities.
The difference is that `list_for` accepts an additional filter argument.
This is useful for situations where you have a `1-to-n` relationship between 2 entities and you want to find all entities on the `n` side that share the same foreign key.

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
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

es_entity::entity_id! { UserDocumentId }

# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserDocumentId")]
pub enum UserDocumentEvent {
    Initialized { id: UserDocumentId, owner_id: UserId },
}

# impl IntoEvents<UserDocumentEvent> for NewUserDocument {
#     fn into_events(self) -> EntityEvents<UserDocumentEvent> {
#         EntityEvents::init(
#             self.id,
#             [UserDocumentEvent::Initialized {
#                 id: self.id,
#                 owner_id: self.owner_id,
#             }],
#         )
#     }
# }
# impl TryFromEvents<UserDocumentEvent> for UserDocument {
#     fn try_from_events(events: EntityEvents<UserDocumentEvent>) -> Result<Self, EsEntityError> {
#         Ok(UserDocument { id: events.id().clone(), owner_id: UserId::new(), events })
#     }
# }
pub struct NewUserDocument { id: UserDocumentId, owner_id: UserId }

#[derive(EsEntity)]
pub struct UserDocument {
    pub id: UserDocumentId,
    owner_id: UserId,
    events: EntityEvents<UserDocumentEvent>,
}

#[derive(EsRepo)]
#[es_repo(
    entity = "UserDocument",
    columns(
        // The column name in the schema
        user_id(
            ty = "UserId",
            // generate the `list_for` fn paired with created_at and id
            list_for(by(id, created_at)),
            // The accessor on the `NewUserDocument` type
            create(accessor = "owner_id"),
            // Its immutable - so no need to ever update it
            update(persist = false)
        )
    )
)]
pub struct UserDocuments {
    pool: sqlx::PgPool
}
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let docs = UserDocuments { pool: init_pool().await? };
    // Assume we have an existing user that we can get the id from
    let owner_id = UserId::new();

    // Batch creating a few entities for illustration
    let new_docs = vec![
        NewUserDocument { id: UserDocumentId::new(), owner_id },
        NewUserDocument { id: UserDocumentId::new(), owner_id }
    ];
    docs.create_all(new_docs).await?;

    // The fns have the form `list_for_<filter>_by_<cursor>`
    let docs = docs.list_for_user_id_by_created_at(
        owner_id, Default::default(), Default::default()
    ).await?;

    assert_eq!(docs.entities.len(), 2);

    Ok(())
}
```
