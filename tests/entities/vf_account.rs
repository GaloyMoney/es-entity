#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { VfAccountId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "VfAccountId")]
pub enum VfAccountEvent {
    Initialized { id: VfAccountId, status: String },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct VfAccount {
    pub id: VfAccountId,
    pub status: String,

    events: EntityEvents<VfAccountEvent>,
}

impl TryFromEvents<VfAccountEvent> for VfAccount {
    fn try_from_events(events: EntityEvents<VfAccountEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = VfAccountBuilder::default();
        for event in events.iter_all() {
            match event {
                VfAccountEvent::Initialized { id, status } => {
                    builder = builder.id(*id).status(status.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewVfAccount {
    #[builder(setter(into))]
    pub id: VfAccountId,
    #[builder(setter(into))]
    pub status: String,
}

impl NewVfAccount {
    pub fn builder() -> NewVfAccountBuilder {
        NewVfAccountBuilder::default()
    }
}

impl IntoEvents<VfAccountEvent> for NewVfAccount {
    fn into_events(self) -> EntityEvents<VfAccountEvent> {
        EntityEvents::init(
            self.id,
            [VfAccountEvent::Initialized {
                id: self.id,
                status: self.status,
            }],
        )
    }
}
