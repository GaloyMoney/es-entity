#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { VfAccountId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "VfAccountId")]
pub enum VfAccountEvent {
    Initialized { id: VfAccountId, status: String },
}

#[derive(EsEntity)]
pub struct VfAccount {
    pub id: VfAccountId,
    pub status: String,

    events: EntityEvents<VfAccountEvent>,
}

impl TryFromEvents<VfAccountEvent> for VfAccount {
    fn try_from_events(events: EntityEvents<VfAccountEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = VfAccount::hydrator();
        for event in events.iter_all() {
            match event {
                VfAccountEvent::Initialized { id, status } => {
                    h.id(*id).status(status.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewVfAccount {
    #[builder(into)]
    pub status: String,
}

impl IntoEvents<VfAccountEvent> for NewVfAccount {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<VfAccountEvent, FromRepo>,
    ) -> EntityEvents<VfAccountEvent> {
        let id = builder.id();
        builder.initial_events([VfAccountEvent::Initialized {
            id,
            status: self.status,
        }])
    }
}
