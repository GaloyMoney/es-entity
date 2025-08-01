mod helpers;

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! {
    OrderId ,
    OrderItemId
}

// OrderItem - the nested entity
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "OrderItemId")]
pub enum OrderItemEvent {
    Created {
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
    pub fn update_quantity(&mut self, quantity: i32) -> bool {
        if self.quantity != quantity {
            self.events
                .push(OrderItemEvent::QuantityUpdated { quantity });
            self.quantity = quantity;
            true
        } else {
            false
        }
    }
}

impl TryFromEvents<OrderItemEvent> for OrderItem {
    fn try_from_events(events: EntityEvents<OrderItemEvent>) -> Result<Self, EsEntityError> {
        let mut builder = OrderItemBuilder::default();

        for event in events.iter_all() {
            match event {
                OrderItemEvent::Created {
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
            vec![OrderItemEvent::Created {
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
    Created { id: OrderId, customer_name: String },
    StatusUpdated { status: String },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Order {
    pub id: OrderId,
    pub customer_name: String,
    pub status: String,
    events: EntityEvents<OrderEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    items: Nested<OrderItem>,
}

impl Order {
    pub fn update_status(&mut self, status: String) -> bool {
        if self.status != status {
            self.events.push(OrderEvent::StatusUpdated {
                status: status.clone(),
            });
            self.status = status;
            true
        } else {
            false
        }
    }

    pub fn add_item(&mut self, item: NewOrderItem) {
        self.items.add_new(item);
    }

    pub fn get_item(&self, item_id: &OrderItemId) -> Option<&OrderItem> {
        self.items.get_persisted(item_id)
    }

    pub fn get_item_mut(&mut self, item_id: &OrderItemId) -> Option<&mut OrderItem> {
        self.items.get_persisted_mut(item_id)
    }

    // @ claude this should return the item
    pub fn item_with_name(&self, product_name: &str) -> Option<&OrderItem> {
        self.items
            .entities()
            .values()
            .find(|item| item.product_name == product_name)
    }

    pub fn n_items(&self) -> usize {
        self.items.entities().len()
    }
}

impl TryFromEvents<OrderEvent> for Order {
    fn try_from_events(events: EntityEvents<OrderEvent>) -> Result<Self, EsEntityError> {
        let mut builder = OrderBuilder::default();
        builder = builder.status("pending".to_string());

        for event in events.iter_all() {
            match event {
                OrderEvent::Created { id, customer_name } => {
                    builder = builder.id(*id).customer_name(customer_name.clone());
                }
                OrderEvent::StatusUpdated { status } => {
                    builder = builder.status(status.clone());
                }
            }
        }

        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewOrder {
    pub id: OrderId,
    pub customer_name: String,
    pub status: String,
}

impl NewOrder {
    pub fn builder() -> NewOrderBuilder {
        NewOrderBuilder::default()
    }
}

impl IntoEvents<OrderEvent> for NewOrder {
    fn into_events(self) -> EntityEvents<OrderEvent> {
        EntityEvents::init(
            self.id,
            vec![OrderEvent::Created {
                id: self.id,
                customer_name: self.customer_name.clone(),
            }],
        )
    }
}

// Repositories
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    err = "es_entity::EsRepoError",
    columns(
        order_id(ty = "OrderId", list_by, parent),
        product_name(ty = "String"),
        quantity(ty = "i32", update(persist = true)),
        price(ty = "f64")
    )
)]
pub struct OrderItems {
    pool: PgPool,
}

impl OrderItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Order",
    err = "es_entity::EsRepoError",
    columns(customer_name = "String", status = "String")
)]
pub struct Orders {
    pool: PgPool,

    #[es_repo(nested)]
    items: OrderItems,
}

impl Orders {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            items: OrderItems::new(pool),
        }
    }
}

// Tests
#[tokio::test]
async fn create_order_with_items() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool);

    // Create a new order with items
    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default()
        .id(order_id)
        .customer_name("Alice Johnson".to_string())
        .status("pending".to_string())
        .build()
        .unwrap();

    let items = vec![
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Laptop".to_string())
            .quantity(1)
            .price(999.99)
            .build()
            .unwrap(),
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Mouse".to_string())
            .quantity(2)
            .price(29.99)
            .build()
            .unwrap(),
    ];

    let mut order = orders.create(new_order).await?;

    // Add items to the order
    for item in items {
        order.add_item(item);
    }

    // Update to persist the nested items
    orders.update(&mut order).await?;

    // Load the order with items
    let loaded_order = orders.find_by_id(order_id).await?;

    assert_eq!(loaded_order.customer_name, "Alice Johnson");

    // Verify items are automatically loaded
    assert_eq!(loaded_order.n_items(), 2);
    assert!(loaded_order.item_with_name("Laptop").is_some());
    assert!(loaded_order.item_with_name("Mouse").is_some());

    Ok(())
}

#[tokio::test]
async fn update_order_and_items() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool);

    // Create order with item
    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default()
        .id(order_id)
        .customer_name("Bob Smith".to_string())
        .status("pending".to_string())
        .build()
        .unwrap();

    let items = vec![
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Keyboard".to_string())
            .quantity(1)
            .price(79.99)
            .build()
            .unwrap(),
    ];

    let mut order = orders.create(new_order).await?;

    // Add items to the order
    for item in items {
        order.add_item(item);
    }

    // Update to persist the nested items
    orders.update(&mut order).await?;

    // Load and update
    let mut loaded_order = orders.find_by_id(order_id).await?;

    // Update order status
    loaded_order.update_status("processing".to_string());

    // Update item quantity using item name
    let keyboard_id = { loaded_order.item_with_name("Keyboard").unwrap().id };
    loaded_order
        .get_item_mut(&keyboard_id)
        .unwrap()
        .update_quantity(3);

    // Add another item
    let new_item = NewOrderItemBuilder::default()
        .id(OrderItemId::new())
        .order_id(order_id)
        .product_name("Monitor".to_string())
        .quantity(1)
        .price(299.99)
        .build()
        .unwrap();

    loaded_order.add_item(new_item);

    orders.update(&mut loaded_order).await?;

    // Verify updates
    let final_order = orders.find_by_id(order_id).await?;

    assert_eq!(final_order.status, "processing");

    // Verify items
    assert_eq!(final_order.n_items(), 2);
    let keyboard = final_order.item_with_name("Keyboard").unwrap();
    assert_eq!(keyboard.quantity, 3);

    Ok(())
}
