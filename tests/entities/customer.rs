#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { CustomerId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "CustomerId")]
pub enum CustomerEvent {
    Initialized {
        id: CustomerId,
        name: Forgettable<String>,
        email: String,
    },
    NameUpdated {
        name: Forgettable<String>,
    },
    EmailUpdated {
        email: String,
    },
    /// Client-declared erasure event (convention): staged before `forget()`
    /// so the erasure consumes a sequence number (fencing stale writers) and
    /// is recorded in the event stream.
    Forgot {},
}

#[derive(EsEntity)]
pub struct Customer {
    pub id: CustomerId,
    pub name: String,
    pub email: String,

    events: EntityEvents<CustomerEvent>,
}

impl Customer {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        self.name = new_name.clone();
        self.events.push(CustomerEvent::NameUpdated {
            name: Forgettable::new(new_name),
        });
        Idempotent::Executed(())
    }

    pub fn update_email(&mut self, new_email: impl Into<String>) -> Idempotent<()> {
        let new_email = new_email.into();
        self.email = new_email.clone();
        self.events
            .push(CustomerEvent::EmailUpdated { email: new_email });
        Idempotent::Executed(())
    }

    /// Stages the domain erasure event. Call before `forget()` so the
    /// erasure consumes a sequence number and lands in the event stream.
    pub fn record_erasure(&mut self) {
        self.events.push(CustomerEvent::Forgot {});
    }
}

impl TryFromEvents<CustomerEvent> for Customer {
    fn try_from_events(events: EntityEvents<CustomerEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Customer::hydrator();
        for event in events.iter_all() {
            match event {
                CustomerEvent::Initialized { id, name, email } => {
                    h.id(*id)
                        .name(
                            name.value()
                                .map(|r| r.clone())
                                .unwrap_or_else(|| "[forgotten]".into()),
                        )
                        .email(email.clone());
                }
                CustomerEvent::NameUpdated { name } => {
                    if let Some(n) = name.value() {
                        h.name(n.clone());
                    }
                }
                CustomerEvent::EmailUpdated { email } => {
                    h.email(email.clone());
                }
                // Erasure event (client convention): payloads were deleted
                // at this point in the stream. No state to apply.
                CustomerEvent::Forgot { .. } => {}
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewCustomer {
    #[builder(into)]
    pub name: String,
    #[builder(into)]
    pub email: String,
}

impl IntoEvents<CustomerEvent> for NewCustomer {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<CustomerEvent, FromRepo>,
    ) -> EntityEvents<CustomerEvent> {
        let id = builder.id();
        builder.initial_events([CustomerEvent::Initialized {
            id,
            name: Forgettable::new(self.name),
            email: self.email,
        }])
    }
}
