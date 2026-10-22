#![allow(dead_code)]

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

#[derive(EsEntity)]
pub struct Contact {
    pub id: ContactId,
    pub partner_id: PartnerId,
    pub email: String,
    pub status: String,

    events: EntityEvents<ContactEvent>,
}

impl TryFromEvents<ContactEvent> for Contact {
    fn try_from_events(events: EntityEvents<ContactEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Contact::hydrator();
        for event in events.iter_all() {
            match event {
                ContactEvent::Initialized {
                    id,
                    partner_id,
                    email,
                    status,
                } => {
                    h.id(*id)
                        .partner_id(*partner_id)
                        .email(email.clone())
                        .status(status.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewContact {
    #[builder(into)]
    pub partner_id: PartnerId,
    #[builder(into)]
    pub email: String,
    #[builder(into)]
    pub status: String,
}

impl IntoEvents<ContactEvent> for NewContact {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<ContactEvent, FromRepo>,
    ) -> EntityEvents<ContactEvent> {
        let id = builder.id();
        builder.initial_events([ContactEvent::Initialized {
            id,
            partner_id: self.partner_id,
            email: self.email,
            status: self.status,
        }])
    }
}
