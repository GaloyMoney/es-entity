use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { CustomerId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "CustomerId")]
pub enum CustomerEvent {
    Initialized { id: CustomerId, name: String },
    NameUpdated { name: String },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Customer {
    pub id: CustomerId,
    pub name: String,

    events: EntityEvents<CustomerEvent>,
}

impl Customer {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        idempotency_guard!(
            self.events.iter_all().rev(),
            CustomerEvent::NameUpdated { name } if name == &new_name,
            => CustomerEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events
            .push(CustomerEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<CustomerEvent> for Customer {
    fn try_from_events(events: EntityEvents<CustomerEvent>) -> Result<Self, EsEntityError> {
        let mut builder = CustomerBuilder::default();
        for event in events.iter_all() {
            match event {
                CustomerEvent::Initialized { id, name } => {
                    builder = builder.id(*id).name(name.clone());
                }
                CustomerEvent::NameUpdated { name } => {
                    builder = builder.name(name.clone());
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
                name: self.name,
            }],
        )
    }
}
