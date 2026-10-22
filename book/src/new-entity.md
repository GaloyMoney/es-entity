# New Entity

The `NewEntity` type represents the data of the `Entity` in a pre-persisted state.
It gets passed to the `Repository::create` function where the `IntoEvents` trait emits the initial `EntityEvent`s which are then persisted and used to hydrate the actual `Entity`.

It is also recommended that any validation of the initial attributes is performed on this type to ensure that the `Entity` will be in a legal state once it gets hydrated for the first time.
A [bon](https://docs.rs/bon/latest/bon/) builder over a fallible `new` is recommended for this purpose: validation lives in `new`, and a missing field is a compile error.

By default the repository generates the id of every entity it creates, so the
`NewEntity` has no id field and `IntoEvents` reads the resolved id from the
builder it receives. An entity whose id is deterministic (derived from another
aggregate, a well-known constant) takes it from the caller instead
([below](#ids-from-the-caller)); the repository declares which, see
[id](./repo-id.md).

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate bon;
# use serde::{Deserialize, Serialize};
# use es_entity::*;
# es_entity::entity_id! { UserId };
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
# }
const MAX_NAME_LENGTH: usize = 100;

#[derive(Debug)]
pub struct NameTooLong;

#[derive(Debug)]
pub struct NewUser {
    pub name: String,
}

#[bon::bon]
impl NewUser {
    // Validation that ensures the initial `Entity` is legal.
    // `build()` returns `Result<NewUser, NameTooLong>`.
    #[builder]
    pub fn new(#[builder(into)] name: String) -> Result<Self, NameTooLong> {
        if name.len() > MAX_NAME_LENGTH {
            return Err(NameTooLong);
        }
        Ok(Self { name })
    }
}

impl IntoEvents<UserEvent> for NewUser {
    // The repository generates the id; `NewUser` has no id field.
    type IdSource = FromRepo;

    // This `fn` returns the first `Events` of the `Entity`
    fn into_events(self, builder: EntityEventsBuilder<UserEvent, FromRepo>) -> EntityEvents<UserEvent> {
        let id = builder.id();
        builder.initial_events([UserEvent::Initialized {
            id,
            name: self.name,
        }])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_creation() {
        let new_user = NewUser::builder().name("Steven").build();
        assert!(new_user.is_ok());
    }

    #[test]
    fn name_too_long() {
        let new_user = NewUser::builder().name("x".repeat(MAX_NAME_LENGTH + 1)).build();
        assert!(new_user.is_err());
    }
}
```

A `FromRepo` `NewEntity` has no route by which a caller-chosen id reaches the
insert: `builder.initial_events` takes no id and the builder cannot be built by
hand.

## Ids from the caller

A `NewEntity` with a deterministic id declares `type IdSource = FromCaller`,
carries the id and passes it to `initial_events`:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# use serde::{Deserialize, Serialize};
# use es_entity::*;
# es_entity::entity_id! { CustomerId };
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "CustomerId")]
# pub enum CustomerEvent {
#     Initialized { id: CustomerId },
# }
pub struct NewCustomer {
    pub id: CustomerId,
}

impl IntoEvents<CustomerEvent> for NewCustomer {
    type IdSource = FromCaller;

    fn into_events(self, builder: EntityEventsBuilder<CustomerEvent, FromCaller>) -> EntityEvents<CustomerEvent> {
        builder.initial_events(self.id, [CustomerEvent::Initialized { id: self.id }])
    }
}
```

Which of the two a `NewEntity` must use is declared on its repository, see
[id](./repo-id.md).
