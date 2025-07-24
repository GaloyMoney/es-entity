# New Entity

The `NewEntity` type represents the data of the `Entity` in a pre-persisted state.
It gets passed to the `Repository::create` function where the `IntoEvents` trait emits the initial `EntityEvent`s which are then persisted and used to hydrate the actual `Entity`.

It is also recommended that any validation of the initial attributes is performed on this type to ensure that the `Entity` will be in a legal state once it gets hydrated for the first time.
Using the [derive_builder](https://docs.rs/derive_builder/latest/derive_builder/) crate is recommended for this purpose.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# use serde::{Deserialize, Serialize};
# use es_entity::*;
# use derive_builder::Builder;
# es_entity::entity_id! { UserId };
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
# }
const MAX_NAME_LENGTH: usize = 100;

#[derive(Debug, Builder)]
// Specify the `validation` fn
#[builder(build_fn(validate = "Self::validate"))]
pub struct NewUser {
    #[builder(setter(into))]
    pub id: UserId,
    #[builder(setter(into))]
    pub name: String,
}

impl NewUser {
    pub fn builder() -> NewUserBuilder {
        NewUserBuilder::default()
    }
}
impl NewUserBuilder {
    // Execute some validation that ensures the initial `Entity` is legal
    fn validate(&self) -> Result<(), String> {
        if self.name.as_ref().expect("name wasn't set").len() > MAX_NAME_LENGTH {
            return Err("Name length exceeded".to_string());
        }
        Ok(())
    }
}

impl IntoEvents<UserEvent> for NewUser {
    // This `fn` returns the first `Events` of the `Entity`
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

#[cfg(test)]
mod tests {
    #[test]
    fn user_creation() {
        let new_user = NewUser::builder().id("user-id").name("Steven").build();
        assert!(new_user.is_ok());
    }
}
```
