#![allow(dead_code)]

use derive_builder::Builder;
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
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
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
}

impl TryFromEvents<CustomerEvent> for Customer {
    fn try_from_events(events: EntityEvents<CustomerEvent>) -> Result<Self, EsEntityError> {
        let mut builder = CustomerBuilder::default();
        for event in events.iter_all() {
            match event {
                CustomerEvent::Initialized { id, name, email } => {
                    builder = builder
                        .id(*id)
                        .name(
                            name.value()
                                .map(|r| r.clone())
                                .unwrap_or_else(|| "[forgotten]".into()),
                        )
                        .email(email.clone());
                }
                CustomerEvent::NameUpdated { name } => {
                    if let Some(n) = name.value() {
                        builder = builder.name(n.clone());
                    }
                }
                CustomerEvent::EmailUpdated { email } => {
                    builder = builder.email(email.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewCustomer {
    #[builder(setter(into))]
    pub id: CustomerId,
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub email: String,
}

impl NewCustomer {
    pub fn builder() -> NewCustomerBuilder {
        NewCustomerBuilder::default()
    }
}

impl IntoEvents<CustomerEvent> for NewCustomer {
    fn into_events(self) -> EntityEvents<CustomerEvent> {
        EntityEvents::init(
            self.id,
            [CustomerEvent::Initialized {
                id: self.id,
                name: Forgettable::new(self.name),
                email: self.email,
            }],
        )
    }
}
