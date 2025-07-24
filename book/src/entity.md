# Entity

In the Software Engineering community the term `Entity` can refer to many different things.
In the context of `es-entity` it is generally meant in the sense put forward by Domain Driven Design.
Strict adherence to DDD is not mandatory to use `es-entity` but there are a lot of benefits to be had by following these principles.

In DDD entities serve the following purpose:
- execute commands that
  - execute business logic
  - enforce domain invariants
  - mutate state
  - record events (in the context of Event Sourcing)
- supply queries that expose some of the entities state

They often host the most critical code in your application where correctness is of upmost importance.
Ideally they are unit-testable and thus should not be overly coupled to the persistence layer (as they generally are when using just about any ORM library).
The design of `es-entity` is very deliberate in not getting in the way of testability of your `Entities`.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use es_entity::*;

es_entity::entity_id! { UserId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId")]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct User {
    pub id: UserId,
    pub name: String,

    events: EntityEvents<UserEvent>,
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        idempotency_guard!(
            self.events.iter_all().rev(),
            UserEvent::NameUpdated { name } if name == &new_name,
            => UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

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

#[derive(Debug, Builder)]
pub struct NewUser {
    #[builder(setter(into))]
    pub id: UserId,
    #[builder(setter(into))]
    pub name: String,
}

impl IntoEvents<UserEvent> for NewUser {
    fn into_events(self) -> EntityEvents<UserEvent> {
        EntityEvents::init(
            self.id,
            [UserEvent::Initialized {
                id: self.id,
                name: self.name,
            }],
        )
    }
}
```
