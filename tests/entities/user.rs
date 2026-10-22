#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { UserId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId", event_context)]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

#[derive(EsEntity)]
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
            already_applied: UserEvent::NameUpdated { name } if name == &new_name,
            resets_on: UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = User::hydrator();
        for event in events.iter_all() {
            match event {
                UserEvent::Initialized { id, name } => {
                    h.id(*id).name(name.clone());
                }
                UserEvent::NameUpdated { name } => {
                    h.name(name.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewUser {
    #[builder(into)]
    pub name: String,
}

impl IntoEvents<UserEvent> for NewUser {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<UserEvent, FromRepo>,
    ) -> EntityEvents<UserEvent> {
        let id = builder.id();
        builder.initial_events([UserEvent::Initialized {
            id,
            name: self.name,
        }])
    }
}
