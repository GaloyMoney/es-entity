#![allow(dead_code)]

use derive_builder::Builder;
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

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Party {
    pub id: PartyId,
    pub customer_id: Option<CustomerId>,
    pub name: String,

    events: EntityEvents<PartyEvent>,
}

impl TryFromEvents<PartyEvent> for Party {
    fn try_from_events(events: EntityEvents<PartyEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = PartyBuilder::default();
        for event in events.iter_all() {
            match event {
                PartyEvent::Initialized {
                    id,
                    customer_id,
                    name,
                } => {
                    builder = builder.id(*id).customer_id(*customer_id).name(name.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewParty {
    #[builder(setter(into))]
    pub id: PartyId,
    #[builder(setter(into, strip_option), default)]
    pub customer_id: Option<CustomerId>,
    #[builder(setter(into))]
    pub name: String,
}

impl NewParty {
    pub fn builder() -> NewPartyBuilder {
        NewPartyBuilder::default()
    }
}

impl IntoEvents<PartyEvent> for NewParty {
    fn into_events(self) -> EntityEvents<PartyEvent> {
        EntityEvents::init(
            self.id,
            [PartyEvent::Initialized {
                id: self.id,
                customer_id: self.customer_id,
                name: self.name,
            }],
        )
    }
}
