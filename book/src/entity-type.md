# Entity Type

The `Entity` type is a struct that holds the `events: EntityEvents<EntityEvent>` field.
Mutations of the entity append events to this collection.
The `Entity` is (re-)constructed from events via the `TryFromEvents` trait.
Other than the `events` field additional fields can be exposed and populated during `hydration`.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# use derive_builder::Builder;
# use serde::{Deserialize, Serialize};
# use es_entity::*;
# es_entity::entity_id! { UserId };
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
# }
# pub struct NewUser { id: UserId, name: String }
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
// Using derive_builder is optional but useful for hydrating
// in the `TryFromEvents` trait.
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,

    // The `events` container - mandatory field.
    // Basically its a `Vec` wrapper with some ES specific augmentation.
    events: EntityEvents<UserEvent>,

    // Marker if you use a name other than `events`.
    // #[es_entity(events)]
    // different_name_for_events_field: EntityEvents<UserEvent>
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        // The idempotency_guard macro is a helper to return quickly
        // if a mutation has already been applied.
        // It is not mandatory but very useful in the context of distributed / multi-thread
        // systems to protect against replays.
        idempotency_guard!(
            self.events.iter_persisted().rev(),
            // If this pattern matches return Idempotent::AlreadyApplied
            UserEvent::NameUpdated { name } if name == &new_name,
            // Stop searching here
            => UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

// Any EsEntity must implement `TryFromEvents`.
// This trait is what hydrates entities after loading the events from the database
impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
        let mut builder = UserBuilder::default();
        for event in events.iter_all() {
            match event {
                UserEvent::Initialized { id, name } => {
                    builder = builder.id(*id).name(name.clone());
                }
                UserEvent::NameUpdated { name } => {
                    builder = builder.name(name.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[cfg(test)]
mod tests {
    fn fresh_user() -> User {
        let id = UserId::new();
        let initial_events = EntityEvents::init(
            id,
            [UserEvent::Initialized {
                id, 
                name: "Willson"
            }],
        );
        User::try_from_events(initial_events).expect("Could not create user");
    }

    #[test]
    fn update_name() {
        let mut user = fresh_user();

        // There are no new events to persist after hydrating
        assert!(!user.events.any_new());

        let new_name = "Gavin".to_string();
        user.update_name(new_name.clone()).unwrap();
        assert_eq!(user.name, new_name);

        // Mutations are expected to append events
        assert!(user.events.any_new());
    }
}

```
