#![allow(dead_code)]

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

#[derive(EsEntity)]
pub struct Transfer {
    pub id: TransferId,
    pub account_id: AccountId,
    pub status: String,
    pub reference: Option<String>,
    pub score: Option<i32>,

    events: EntityEvents<TransferEvent>,
}

impl TryFromEvents<TransferEvent> for Transfer {
    fn try_from_events(events: EntityEvents<TransferEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Transfer::hydrator();
        for event in events.iter_all() {
            match event {
                TransferEvent::Initialized {
                    id,
                    account_id,
                    status,
                    reference,
                    score,
                } => {
                    h.id(*id)
                        .account_id(*account_id)
                        .status(status.clone())
                        .maybe_reference(reference.clone())
                        .maybe_score(*score);
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewTransfer {
    #[builder(into)]
    pub account_id: AccountId,
    #[builder(into)]
    pub status: String,
    #[builder(into)]
    pub reference: Option<String>,
    pub score: Option<i32>,
}

impl IntoEvents<TransferEvent> for NewTransfer {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<TransferEvent, FromRepo>,
    ) -> EntityEvents<TransferEvent> {
        let id = builder.id();
        builder.initial_events([TransferEvent::Initialized {
            id,
            account_id: self.account_id,
            status: self.status,
            reference: self.reference,
            score: self.score,
        }])
    }
}
