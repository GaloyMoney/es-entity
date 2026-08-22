#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { FacilityId }
es_entity::entity_id! { PartnerId }
es_entity::entity_id! { CustomerId }

/// Mirrors lana's multi-API shape: a `CreditFacility`-like entity read by an
/// admin API (`All`), a partner API (`partner_id` scope) and a customer API
/// (`customer_id` scope) — two independent, disjunctive scope dimensions on
/// the same repo.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "FacilityId")]
pub enum FacilityEvent {
    Initialized {
        id: FacilityId,
        partner_id: PartnerId,
        customer_id: CustomerId,
        status: String,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Facility {
    pub id: FacilityId,
    pub partner_id: PartnerId,
    pub customer_id: CustomerId,
    pub status: String,

    events: EntityEvents<FacilityEvent>,
}

impl TryFromEvents<FacilityEvent> for Facility {
    fn try_from_events(events: EntityEvents<FacilityEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = FacilityBuilder::default();
        for event in events.iter_all() {
            match event {
                FacilityEvent::Initialized {
                    id,
                    partner_id,
                    customer_id,
                    status,
                } => {
                    builder = builder
                        .id(*id)
                        .partner_id(*partner_id)
                        .customer_id(*customer_id)
                        .status(status.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewFacility {
    #[builder(setter(into))]
    pub id: FacilityId,
    #[builder(setter(into))]
    pub partner_id: PartnerId,
    #[builder(setter(into))]
    pub customer_id: CustomerId,
    #[builder(setter(into))]
    pub status: String,
}

impl NewFacility {
    pub fn builder() -> NewFacilityBuilder {
        NewFacilityBuilder::default()
    }
}

impl IntoEvents<FacilityEvent> for NewFacility {
    fn into_events(self) -> EntityEvents<FacilityEvent> {
        EntityEvents::init(
            self.id,
            [FacilityEvent::Initialized {
                id: self.id,
                partner_id: self.partner_id,
                customer_id: self.customer_id,
                status: self.status,
            }],
        )
    }
}
