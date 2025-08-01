mod helpers;

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;

// Define IDs using the macro
es_entity::entity_id! { OrderId }
es_entity::entity_id! { OrderItemId }

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

#[derive(EsEntity, Clone, Builder)]
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

#[derive(EsEntity, Clone, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Order {
    pub id: OrderId,
    pub customer_name: String,
    pub status: String,
    events: EntityEvents<OrderEvent>,
    items: HashMap<OrderItemId, OrderItem>,
    new_items: Vec<NewOrderItem>,
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

    pub fn add_new_item(&mut self, item: NewOrderItem) {
        self.new_items.push(item);
    }

    pub fn items(&self) -> &HashMap<OrderItemId, OrderItem> {
        &self.items
    }

    pub fn items_mut(&mut self) -> &mut HashMap<OrderItemId, OrderItem> {
        &mut self.items
    }

    pub fn new_items_mut(&mut self) -> &mut Vec<NewOrderItem> {
        &mut self.new_items
    }

    pub fn add_item(&mut self, item: OrderItem) {
        self.items.insert(item.id, item);
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

        builder
            .events(events)
            .items(HashMap::new())
            .new_items(Vec::new())
            .build()
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
        order_id(ty = "OrderId", list_by),
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
    columns(
        customer_name(ty = "String", create(accessor = "customer_name.clone()")),
        status(
            ty = "String",
            create(accessor = "status.clone()"),
            update(persist = true)
        )
    )
)]
pub struct Orders {
    pool: PgPool,
}

impl Orders {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn create_order_items<OP>(
        &self,
        op: &mut OP,
        order: &mut Order,
    ) -> Result<(), EsRepoError>
    where
        OP: for<'o> es_entity::AtomicOperation<'o>,
    {
        let order_items = OrderItems::new(self.pool.clone());

        let new_items = order.new_items_mut().drain(..).collect::<Vec<_>>();
        for new_item in new_items {
            let item = order_items.create_in_op(op, new_item).await?;
            order.add_item(item);
        }

        Ok(())
    }

    async fn update_order_items<OP>(
        &self,
        op: &mut OP,
        order: &mut Order,
    ) -> Result<(), EsRepoError>
    where
        OP: for<'o> es_entity::AtomicOperation<'o>,
    {
        let order_items = OrderItems::new(self.pool.clone());

        // Update existing items
        for item in order.items_mut().values_mut() {
            order_items.update_in_op(op, item).await?;
        }

        // Create new items
        self.create_order_items(op, order).await?;

        Ok(())
    }

    pub async fn create_with_items(
        &self,
        new_order: NewOrder,
        items: Vec<NewOrderItem>,
    ) -> Result<Order, EsRepoError> {
        let mut op = self.begin_op().await?;
        let mut order = self.create_in_op(&mut op, new_order).await?;

        for item in items {
            order.add_new_item(item);
        }

        self.create_order_items(&mut op, &mut order).await?;
        op.commit().await?;

        Ok(order)
    }

    pub async fn update_with_items(&self, order: &mut Order) -> Result<(), EsRepoError> {
        let mut op = self.begin_op().await?;
        self.update_in_op(&mut op, order).await?;
        self.update_order_items(&mut op, order).await?;
        op.commit().await?;
        Ok(())
    }

    pub async fn find_with_items(&self, id: OrderId) -> Result<Order, EsRepoError> {
        let mut order = self.find_by_id(id).await?;

        // Load items
        let order_items = OrderItems::new(self.pool.clone());
        let query_result = order_items
            .list_by_order_id(
                PaginatedQueryArgs {
                    first: 100,
                    after: None,
                },
                ListDirection::Descending,
            )
            .await?;
        let items = query_result.entities;

        for item in items {
            if item.order_id == id {
                order.add_item(item);
            }
        }

        Ok(order)
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

    let _order = orders.create_with_items(new_order, items).await?;

    // Load the order with items
    let loaded_order = orders.find_with_items(order_id).await?;

    assert_eq!(loaded_order.customer_name, "Alice Johnson");
    assert_eq!(loaded_order.items().len(), 2);

    // Verify items
    let items: Vec<&OrderItem> = loaded_order.items().values().collect();
    assert!(items.iter().any(|item| item.product_name == "Laptop"));
    assert!(items.iter().any(|item| item.product_name == "Mouse"));

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

    orders.create_with_items(new_order, items).await?;

    // Load and update
    let mut loaded_order = orders.find_with_items(order_id).await?;

    // Update order status
    loaded_order.update_status("processing".to_string());

    // Update item quantity
    if let Some(item) = loaded_order.items_mut().values_mut().next() {
        item.update_quantity(3);
    }

    // Add another item
    let new_item = NewOrderItemBuilder::default()
        .id(OrderItemId::new())
        .order_id(order_id)
        .product_name("Monitor".to_string())
        .quantity(1)
        .price(299.99)
        .build()
        .unwrap();

    loaded_order.add_new_item(new_item);

    orders.update_with_items(&mut loaded_order).await?;

    // Verify updates
    let final_order = orders.find_with_items(order_id).await?;

    assert_eq!(final_order.status, "processing");
    assert_eq!(final_order.items().len(), 2);

    let keyboard = final_order
        .items()
        .values()
        .find(|item| item.product_name == "Keyboard")
        .unwrap();
    assert_eq!(keyboard.quantity, 3);

    Ok(())
}
