#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { ProfileId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "ProfileId")]
pub enum ProfileEvent {
    Initialized {
        id: ProfileId,
        name: String,
        email: String,
    },
    NameUpdated {
        name: String,
    },
    EmailUpdated {
        email: String,
    },
}

#[derive(Debug)]
pub struct ProfileData {
    pub name: String,
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Profile {
    pub id: ProfileId,
    pub data: ProfileData,
    pub email: String,

    events: EntityEvents<ProfileEvent>,
}

impl Profile {
    pub fn display_name(&self) -> String {
        format!("Profile: {}", self.data.name)
    }

    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        idempotency_guard!(
            self.events.iter_all().rev(),
            ProfileEvent::NameUpdated { name } if name == &new_name,
            => ProfileEvent::NameUpdated { .. }
        );

        self.data.name = new_name.clone();
        self.events
            .push(ProfileEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }

    pub fn update_email(&mut self, new_email: impl Into<String>) -> Idempotent<()> {
        let new_email = new_email.into();
        idempotency_guard!(
            self.events.iter_all().rev(),
            ProfileEvent::EmailUpdated { email } if email == &new_email,
            => ProfileEvent::EmailUpdated { .. }
        );

        self.email = new_email.clone();
        self.events
            .push(ProfileEvent::EmailUpdated { email: new_email });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<ProfileEvent> for Profile {
    fn try_from_events(events: EntityEvents<ProfileEvent>) -> Result<Self, EsEntityError> {
        let mut builder = ProfileBuilder::default();
        for event in events.iter_all() {
            match event {
                ProfileEvent::Initialized { id, name, email } => {
                    builder = builder
                        .id(*id)
                        .data(ProfileData { name: name.clone() })
                        .email(email.clone());
                }
                ProfileEvent::NameUpdated { name } => {
                    builder = builder.data(ProfileData { name: name.clone() });
                }
                ProfileEvent::EmailUpdated { email } => {
                    builder = builder.email(email.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewProfile {
    #[builder(setter(into))]
    pub id: ProfileId,
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub email: String,
}

impl NewProfile {
    pub fn builder() -> NewProfileBuilder {
        NewProfileBuilder::default()
    }

    pub fn display_name(&self) -> String {
        format!("Profile: {}", self.name)
    }
}

impl IntoEvents<ProfileEvent> for NewProfile {
    fn into_events(self) -> EntityEvents<ProfileEvent> {
        EntityEvents::init(
            self.id,
            [ProfileEvent::Initialized {
                id: self.id,
                name: self.name,
                email: self.email,
            }],
        )
    }
}
