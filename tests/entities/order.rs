#![allow(dead_code)]

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! {
    OrderId ,
    OrderItemId
}

// OrderItem - the nested entity
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "OrderItemId")]
pub enum OrderItemEvent {
    Initialized {
        id: OrderItemId,
        order_id: OrderId,
        product_name: String,
        quantity: i32,
        price: f64,
    },
    QuantityUpdated {
        quantity: i32,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct OrderItem {
    pub id: OrderItemId,
    pub order_id: OrderId,
    pub product_name: String,
    pub quantity: i32,
    pub price: f64,
    events: EntityEvents<OrderItemEvent>,
}

impl OrderItem {
    pub fn update_quantity(&mut self, quantity: i32) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            OrderItemEvent::QuantityUpdated { quantity: q } if q == &quantity,
            => OrderItemEvent::QuantityUpdated { .. }
        );

        self.quantity = quantity;
        self.events
            .push(OrderItemEvent::QuantityUpdated { quantity });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<OrderItemEvent> for OrderItem {
    fn try_from_events(events: EntityEvents<OrderItemEvent>) -> Result<Self, EsEntityError> {
        let mut builder = OrderItemBuilder::default();

        for event in events.iter_all() {
            match event {
                OrderItemEvent::Initialized {
                    id,
                    order_id,
                    product_name,
                    quantity,
                    price,
                } => {
                    builder = builder
                        .id(*id)
                        .order_id(*order_id)
                        .product_name(product_name.clone())
                        .quantity(*quantity)
                        .price(*price);
                }
                OrderItemEvent::QuantityUpdated { quantity } => {
                    builder = builder.quantity(*quantity);
                }
            }
        }

        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewOrderItem {
    pub id: OrderItemId,
    pub order_id: OrderId,
    #[builder(setter(into))]
    pub product_name: String,
    pub quantity: i32,
    pub price: f64,
}

impl NewOrderItem {
    pub fn builder() -> NewOrderItemBuilder {
        NewOrderItemBuilder::default()
    }
}

impl IntoEvents<OrderItemEvent> for NewOrderItem {
    fn into_events(self) -> EntityEvents<OrderItemEvent> {
        EntityEvents::init(
            self.id,
            vec![OrderItemEvent::Initialized {
                id: self.id,
                order_id: self.order_id,
                product_name: self.product_name,
                quantity: self.quantity,
                price: self.price,
            }],
        )
    }
}

// Order - the parent entity
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "OrderId")]
pub enum OrderEvent {
    Initialized { id: OrderId },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Order {
    pub id: OrderId,
    events: EntityEvents<OrderEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    items: Nested<OrderItem>,
}

impl Order {
    pub fn add_item(&mut self, item: NewOrderItem) {
        self.items.add_new(item);
    }

    pub fn find_item_with_name(&self, product_name: &str) -> Option<&OrderItem> {
        self.items
            .entities()
            .values()
            .find(|item| item.product_name == product_name)
    }

    pub fn n_items(&self) -> usize {
        self.items.entities().len()
    }

    pub fn update_item_quantity(&mut self, product_name: &str, quantity: i32) -> Idempotent<()> {
        if let Some(item) = self
            .items
            .entities_mut()
            .values_mut()
            .find(|item| item.product_name == product_name)
        {
            item.update_quantity(quantity)
        } else {
            Idempotent::Ignored
        }
    }
}

impl TryFromEvents<OrderEvent> for Order {
    fn try_from_events(events: EntityEvents<OrderEvent>) -> Result<Self, EsEntityError> {
        let mut builder = OrderBuilder::default();

        for event in events.iter_all() {
            match event {
                OrderEvent::Initialized { id } => builder = builder.id(*id),
            }
        }

        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewOrder {
    pub id: OrderId,
}

impl NewOrder {
    pub fn builder() -> NewOrderBuilder {
        NewOrderBuilder::default()
    }
}

impl IntoEvents<OrderEvent> for NewOrder {
    fn into_events(self) -> EntityEvents<OrderEvent> {
        EntityEvents::init(self.id, vec![OrderEvent::Initialized { id: self.id }])
    }
}
