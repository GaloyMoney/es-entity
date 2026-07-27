#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { TransferId }
es_entity::entity_id! { AccountId }

/// Mirrors the shape of lana's `Disbursal` repo: two non-optional `list_for`
/// filter columns (`account_id`, `status`), one optional filter column
/// (`reference`), and a nullable sort column (`score`) for NULL-cursor edge
/// cases.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "TransferId")]
pub enum TransferEvent {
    Initialized {
        id: TransferId,
        account_id: AccountId,
        status: String,
        reference: Option<String>,
        score: Option<i32>,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Transfer {
    pub id: TransferId,
    pub account_id: AccountId,
    pub status: String,
    #[builder(default)]
    pub reference: Option<String>,
    #[builder(default)]
    pub score: Option<i32>,

    events: EntityEvents<TransferEvent>,
}

impl TryFromEvents<TransferEvent> for Transfer {
    fn try_from_events(events: EntityEvents<TransferEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = TransferBuilder::default();
        for event in events.iter_all() {
            match event {
                TransferEvent::Initialized {
                    id,
                    account_id,
                    status,
                    reference,
                    score,
                } => {
                    builder = builder
                        .id(*id)
                        .account_id(*account_id)
                        .status(status.clone())
                        .reference(reference.clone())
                        .score(*score);
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewTransfer {
    #[builder(setter(into))]
    pub id: TransferId,
    #[builder(setter(into))]
    pub account_id: AccountId,
    #[builder(setter(into))]
    pub status: String,
    #[builder(setter(into, strip_option), default)]
    pub reference: Option<String>,
    #[builder(default)]
    pub score: Option<i32>,
}

impl NewTransfer {
    pub fn builder() -> NewTransferBuilder {
        NewTransferBuilder::default()
    }
}

impl IntoEvents<TransferEvent> for NewTransfer {
    fn into_events(self) -> EntityEvents<TransferEvent> {
        EntityEvents::init(
            self.id,
            [TransferEvent::Initialized {
                id: self.id,
                account_id: self.account_id,
                status: self.status,
                reference: self.reference,
                score: self.score,
            }],
        )
    }
}
