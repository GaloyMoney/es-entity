#![allow(dead_code)]

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

#[derive(EsEntity)]
pub struct Partner {
    pub id: PartnerId,
    pub name: String,

    events: EntityEvents<PartnerEvent>,
}

impl TryFromEvents<PartnerEvent> for Partner {
    fn try_from_events(events: EntityEvents<PartnerEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Partner::hydrator();
        for event in events.iter_all() {
            match event {
                PartnerEvent::Initialized { id, name } => {
                    h.id(*id).name(name.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewPartner {
    #[builder(into)]
    pub id: PartnerId,
    #[builder(into)]
    pub name: String,
}

impl IntoEvents<PartnerEvent> for NewPartner {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<PartnerEvent, FromCaller>,
    ) -> EntityEvents<PartnerEvent> {
        builder.initial_events(
            self.id,
            [PartnerEvent::Initialized {
                id: self.id,
                name: self.name,
            }],
        )
    }
}
