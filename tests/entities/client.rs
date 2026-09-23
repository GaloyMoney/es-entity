#![allow(dead_code)]

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! { ClientId }

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "ClientId")]
pub enum ClientEvent {
    Initialized {
        id: ClientId,
        email: Forgettable<String>,
    },
    EmailChanged {
        email: Forgettable<String>,
    },
}

#[derive(EsSnapshot, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientSnapshot {
    pub id: ClientId,
    pub email: Forgettable<String>,
    pub changes: u32,
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Client {
    pub id: ClientId,
    events: EntityEvents<ClientEvent, ClientSnapshot>,
}

impl Client {
    pub fn email(&self) -> Option<String> {
        self.events
            .replay()
            .rev()
            .find_map(|r| match r {
                Replay::Event(ClientEvent::EmailChanged { email }) => Some(email.clone()),
                Replay::Event(ClientEvent::Initialized { email, .. }) => Some(email.clone()),
                Replay::Snapshot(s) => Some(s.email.clone()),
            })
            .and_then(|f| f.value().map(|r| (*r).clone()))
    }

    pub fn changes(&self) -> u32 {
        self.events.replay().fold(0, |acc, r| match r {
            Replay::Snapshot(s) => s.changes,
            Replay::Event(ClientEvent::EmailChanged { .. }) => acc + 1,
            Replay::Event(_) => acc,
        })
    }

    pub fn tail_len(&self) -> usize {
        self.events.tail_len()
    }

    pub fn change_email(&mut self, email: impl Into<String>) -> Idempotent<()> {
        let email = email.into();
        idempotency_guard!(
            self.events.replay().rev(),
            already_applied: ClientEvent::EmailChanged { email: e } if e.value().map(|r| &*r == &email).unwrap_or(false),
            snapshot: s if s.email.value().map(|r| &*r == &email).unwrap_or(false),
        );
        self.events.push(ClientEvent::EmailChanged {
            email: Forgettable::new(email),
        });
        Idempotent::Executed(())
    }
}

impl Snapshotting for Client {
    fn snapshot(&self) -> Option<ClientSnapshot> {
        (self.events.tail_len() >= 2).then(|| ClientSnapshot {
            id: self.id,
            email: self
                .email()
                .map(Forgettable::new)
                .unwrap_or_else(Forgettable::forgotten),
            changes: self.changes(),
        })
    }
}

impl TryFromEvents<ClientEvent, ClientSnapshot> for Client {
    fn try_from_events(
        events: EntityEvents<ClientEvent, ClientSnapshot>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = ClientBuilder::default();
        for r in events.replay() {
            match r {
                Replay::Snapshot(s) => builder = builder.id(s.id),
                Replay::Event(ClientEvent::Initialized { id, .. }) => builder = builder.id(*id),
                Replay::Event(_) => {}
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewClient {
    pub id: ClientId,
    #[builder(setter(into))]
    pub email: String,
}

impl NewClient {
    pub fn builder() -> NewClientBuilder {
        NewClientBuilder::default()
    }
}

impl IntoEvents<ClientEvent> for NewClient {
    fn into_events(self) -> EntityEvents<ClientEvent> {
        EntityEvents::init(
            self.id,
            [ClientEvent::Initialized {
                id: self.id,
                email: Forgettable::new(self.email),
            }],
        )
    }
}
