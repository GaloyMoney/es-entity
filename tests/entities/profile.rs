#![allow(dead_code)]

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

#[derive(EsEntity)]
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
            already_applied: ProfileEvent::NameUpdated { name } if name == &new_name,
            resets_on: ProfileEvent::NameUpdated { .. }
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
            already_applied: ProfileEvent::EmailUpdated { email } if email == &new_email,
            resets_on: ProfileEvent::EmailUpdated { .. }
        );

        self.email = new_email.clone();
        self.events
            .push(ProfileEvent::EmailUpdated { email: new_email });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<ProfileEvent> for Profile {
    fn try_from_events(events: EntityEvents<ProfileEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Profile::hydrator();
        for event in events.iter_all() {
            match event {
                ProfileEvent::Initialized { id, name, email } => {
                    h.id(*id)
                        .data(ProfileData { name: name.clone() })
                        .email(email.clone());
                }
                ProfileEvent::NameUpdated { name } => {
                    h.data(ProfileData { name: name.clone() });
                }
                ProfileEvent::EmailUpdated { email } => {
                    h.email(email.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewProfile {
    #[builder(into)]
    pub name: String,
    #[builder(into)]
    pub email: String,
}

impl NewProfile {
    pub fn display_name(&self) -> String {
        format!("Profile: {}", self.name)
    }
}

impl IntoEvents<ProfileEvent> for NewProfile {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<ProfileEvent, FromRepo>,
    ) -> EntityEvents<ProfileEvent> {
        let id = builder.id();
        builder.initial_events([ProfileEvent::Initialized {
            id,
            name: self.name,
            email: self.email,
        }])
    }
}
