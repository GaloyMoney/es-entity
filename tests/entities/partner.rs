#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

pub use super::contact::PartnerId;

/// Mirrors the shape of lana's tenancy-root entity: the rows' scope value is
/// their own `id` — there is no separate tenant column. Scoped repos for such
/// entities mark the implicit id column via `id(scope)`.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "PartnerId")]
pub enum PartnerEvent {
    Initialized { id: PartnerId, name: String },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Partner {
    pub id: PartnerId,
    pub name: String,

    events: EntityEvents<PartnerEvent>,
}

impl TryFromEvents<PartnerEvent> for Partner {
    fn try_from_events(events: EntityEvents<PartnerEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = PartnerBuilder::default();
        for event in events.iter_all() {
            match event {
                PartnerEvent::Initialized { id, name } => {
                    builder = builder.id(*id).name(name.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewPartner {
    #[builder(setter(into))]
    pub id: PartnerId,
    #[builder(setter(into))]
    pub name: String,
}

impl NewPartner {
    pub fn builder() -> NewPartnerBuilder {
        NewPartnerBuilder::default()
    }
}

impl IntoEvents<PartnerEvent> for NewPartner {
    fn into_events(self) -> EntityEvents<PartnerEvent> {
        EntityEvents::init(
            self.id,
            [PartnerEvent::Initialized {
                id: self.id,
                name: self.name,
            }],
        )
    }
}
