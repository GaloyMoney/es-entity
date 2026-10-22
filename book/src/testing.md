# Testing

Tests build entities without a repository through `es_entity::testing`,
enabled by the `testing` feature. Enable it from `[dev-dependencies]` only, so
production builds do not contain it:

```toml
[dev-dependencies]
es-entity = { version = "...", features = ["testing"] }
```

`use es_entity::testing::*;` brings three functions onto every entity:

| Call | Builds the entity from |
|---|---|
| `User::hydrate_from_events(id, [events..])` | any event stream, stored under `id` |
| `User::hydrate_from_new(id, new_user)` | a `NewUser` whose repo generates the id; the test picks it |
| `User::hydrate_from_new(new_user)` | a `NewUser` that supplies its own id |

Each returns `Result<User, EntityHydrationError>` holding the entity a
repository load would return: its events are all persisted, recorded at
`Clock::now()`. Only the events a command pushes afterwards are new.

With the `User` from [Entity Type](./entity-type.md) and the `NewUser` from
[New Entity](./new-entity.md), the body of a `#[test]` in a `#[cfg(test)]`
module reads:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# use serde::{Deserialize, Serialize};
# use es_entity::*;
# es_entity::entity_id! { UserId }
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
# }
# pub struct NewUser { name: String }
# impl IntoEvents<UserEvent> for NewUser {
#     type IdSource = FromRepo;
#     fn into_events(self, builder: EntityEventsBuilder<UserEvent, FromRepo>) -> EntityEvents<UserEvent> {
#         let id = builder.id();
#         builder.initial_events([UserEvent::Initialized { id, name: self.name }])
#     }
# }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     pub name: String,
#     events: EntityEvents<UserEvent>,
# }
# impl User {
#     pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
#         let new_name = new_name.into();
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             already_applied: UserEvent::NameUpdated { name } if name == &new_name,
#             resets_on: UserEvent::NameUpdated { .. }
#         );
#         self.name = new_name.clone();
#         self.events.push(UserEvent::NameUpdated { name: new_name });
#         Idempotent::Executed(())
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         let mut hydrator = User::hydrator();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { id, name } => {
#                     hydrator.id(*id).name(name.clone());
#                 }
#                 UserEvent::NameUpdated { name } => {
#                     hydrator.name(name.clone());
#                 }
#             }
#         }
#         hydrator.build(events)
#     }
# }
use es_entity::testing::*;

# fn main() {
let id = UserId::new();
// `NewUser`'s repo generates the id, so the test picks it.
let mut user = User::hydrate_from_new(id, NewUser { name: "Willson".to_string() }).unwrap();
// Any event stream can be hydrated directly too.
let same = User::hydrate_from_events(id, [UserEvent::Initialized { id, name: "Willson".to_string() }]).unwrap();
assert_eq!(user.name, same.name);
assert!(!user.events().any_new());

let new_name = "Gavin".to_string();
assert!(user.update_name(new_name.clone()).did_execute());
assert_eq!(user.name, new_name);
assert!(user.update_name(new_name).was_already_applied());
# }
```
