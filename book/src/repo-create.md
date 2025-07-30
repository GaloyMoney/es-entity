# Create Function

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
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         unimplemented!()
#     }
# }
use es_entity::*;

#[derive(EsRepo)]
#[es_repo(entity = "User")]
pub struct Users {
    pool: sqlx::PgPool
}
```
