#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { PartyId }
es_entity::entity_id! { CustomerId }

/// Mirrors lana's `core/party` shape: `customer_id` is a **nullable** scope
/// column. `Some(id)` means the party is a customer's own party (owned);
/// `None` means the party belongs to no customer directly (e.g. an
/// organization-member individual party) — such rows must stay invisible to
/// every `Customer(_)` scoped read.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "PartyId")]
pub enum PartyEvent {
    Initialized {
        id: PartyId,
        customer_id: Option<CustomerId>,
        name: String,
    },
}

#[derive(EsEntity)]
pub struct Party {
    pub id: PartyId,
    pub customer_id: Option<CustomerId>,
    pub name: String,

    events: EntityEvents<PartyEvent>,
}

impl TryFromEvents<PartyEvent> for Party {
    fn try_from_events(events: EntityEvents<PartyEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Party::hydrator();
        for event in events.iter_all() {
            match event {
                PartyEvent::Initialized {
                    id,
                    customer_id,
                    name,
                } => {
                    h.id(*id).maybe_customer_id(*customer_id).name(name.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewParty {
    #[builder(into)]
    pub customer_id: Option<CustomerId>,
    #[builder(into)]
    pub name: String,
}

impl IntoEvents<PartyEvent> for NewParty {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<PartyEvent, FromRepo>,
    ) -> EntityEvents<PartyEvent> {
        let id = builder.id();
        builder.initial_events([PartyEvent::Initialized {
            id,
            customer_id: self.customer_id,
            name: self.name,
        }])
    }
}
