# fn delete

If you are using Event Sourcing we assume you believe in immutability and keeping a long term audit history.
Deleting data from the Database goes against these principles.
Therefore `es-entity` does not provide a way to actually delete data.
It is however possible to configure a soft delete option by marking `delete = soft` on the `EsRepo`.

This will omit entities that have been flagged as deleted from all queries as well as generate additional queries that can include the deleted entities:
```rust,ignore
fn find_by_<column>_include_deleted
fn maybe_find_by_<column>_include_deleted
fn list_by_<column>_include_deleted
fn list_for_<column>_by_<cursor>_include_deleted
```

As a prerequisite the `index` table must include a `deleted` column:
```sql
CREATE TABLE users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  -- deleted will be set to 'TRUE' when `delete` is called.
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);
```

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate tokio;
# extern crate anyhow;
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
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
#         Ok(User { id: events.id().clone(), name: "Delyth".to_string(), events })
#     }
# }
# pub struct NewUser { id: UserId, name: String }
use es_entity::*;

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
   Initialized { id: UserId, name: String },
   Deleted
}

#[derive(EsEntity)]
pub struct User {
    pub id: UserId,
    name: String,
    events: EntityEvents<UserEvent>,
}

impl User {
    fn delete(&mut self) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_persisted(),
            UserEvent::Deleted
        );
        self.events.push(UserEvent::Deleted);
        Idempotent::Executed(())
    }
}

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(name = "String"),
    delete = "soft"
)]
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
    let new_user = NewUser { id: UserId::new(), name: "Delyth".to_string() };
    let mut user = users.create(new_user).await?;

    let found_user = users.maybe_find_by_name("Delyth").await?;
    assert!(found_user.is_some());

    if user.delete().did_execute() {
        users.delete(user).await?;
    }

    let found_user = users.maybe_find_by_name("Delyth").await?;
    assert!(found_user.is_none());

    let found_user = users.maybe_find_by_name_include_deleted("Delyth").await?;
    assert!(found_user.is_some());
    
#     sqlx::query!(r#"
#         WITH deleted_users AS (
#             DELETE FROM user_events 
#             WHERE id IN (SELECT id FROM users WHERE deleted = true)
#             RETURNING id
#         )
#         DELETE FROM users WHERE deleted = true"#
#     ).execute(users.pool()).await?;
    Ok(())
}
```
