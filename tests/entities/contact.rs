#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { ContactId }
es_entity::entity_id! { PartnerId }

/// Mirrors the shape of lana's partner-scoped entities: a non-nullable
/// tenant column (`partner_id`) marked `scope`, a unique lookup column
/// (`email`) and a filter column (`status`).
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "ContactId")]
pub enum ContactEvent {
    Initialized {
        id: ContactId,
        partner_id: PartnerId,
        email: String,
        status: String,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Contact {
    pub id: ContactId,
    pub partner_id: PartnerId,
    pub email: String,
    pub status: String,

    events: EntityEvents<ContactEvent>,
}

impl TryFromEvents<ContactEvent> for Contact {
    fn try_from_events(events: EntityEvents<ContactEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = ContactBuilder::default();
        for event in events.iter_all() {
            match event {
                ContactEvent::Initialized {
                    id,
                    partner_id,
                    email,
                    status,
                } => {
                    builder = builder
                        .id(*id)
                        .partner_id(*partner_id)
                        .email(email.clone())
                        .status(status.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewContact {
    #[builder(setter(into))]
    pub id: ContactId,
    #[builder(setter(into))]
    pub partner_id: PartnerId,
    #[builder(setter(into))]
    pub email: String,
    #[builder(setter(into))]
    pub status: String,
}

impl NewContact {
    pub fn builder() -> NewContactBuilder {
        NewContactBuilder::default()
    }
}

impl IntoEvents<ContactEvent> for NewContact {
    fn into_events(self) -> EntityEvents<ContactEvent> {
        EntityEvents::init(
            self.id,
            [ContactEvent::Initialized {
                id: self.id,
                partner_id: self.partner_id,
                email: self.email,
                status: self.status,
            }],
        )
    }
}
