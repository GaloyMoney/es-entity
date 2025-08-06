mod entities;
mod helpers;

use entities::order::*;
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Order")]
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

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    columns(order_id(ty = "OrderId", update(persist = false), parent))
)]
pub struct OrderItems {
    pool: PgPool,
}

impl OrderItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn create_order_and_add_items() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool);

    // Create a new order with items
    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();

    let mut order = orders.create(new_order).await?;
    for item in [
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Laptop")
            .quantity(1)
            .price(999.99)
            .build()
            .unwrap(),
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Mouse")
            .quantity(2)
            .price(29.99)
            .build()
            .unwrap(),
    ] {
        order.add_item(item);
    }

    orders.update(&mut order).await?;

    let loaded_order = orders.find_by_id(order_id).await?;

    // Verify items are automatically loaded
    assert_eq!(loaded_order.n_items(), 2);
    assert!(loaded_order.find_item_with_name("Laptop").is_some());
    assert!(loaded_order.find_item_with_name("Mouse").is_some());

    Ok(())
}

#[tokio::test]
async fn update_item() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool);

    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();
    let mut order = orders.create(new_order).await?;
    order.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id)
            .product_name("Keyboard")
            .quantity(1)
            .price(79.99)
            .build()
            .unwrap(),
    );
    orders.update(&mut order).await?;

    let mut loaded_order = orders.find_by_id(order_id).await?;
    let keyboard = loaded_order.find_item_with_name("Keyboard").unwrap();
    assert_eq!(keyboard.quantity, 1);

    loaded_order.update_item_quantity("Keyboard", 3).unwrap();
    let keyboard = loaded_order.find_item_with_name("Keyboard").unwrap();
    assert_eq!(keyboard.quantity, 3);

    orders.update(&mut loaded_order).await?;

    let final_order = orders.find_by_id(order_id).await?;

    let keyboard = final_order.find_item_with_name("Keyboard").unwrap();
    assert_eq!(keyboard.quantity, 3);

    Ok(())
}

#[tokio::test]
async fn find_all_with_nested_entities() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool);

    // Create multiple orders with different items
    let order_id_1 = OrderId::new();
    let order_id_2 = OrderId::new();
    let order_id_3 = OrderId::new();

    // Create first order with laptop and mouse
    let mut order1 = orders
        .create(NewOrderBuilder::default().id(order_id_1).build().unwrap())
        .await?;
    order1.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id_1)
            .product_name("Laptop")
            .quantity(1)
            .price(999.99)
            .build()
            .unwrap(),
    );
    order1.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id_1)
            .product_name("Mouse")
            .quantity(2)
            .price(29.99)
            .build()
            .unwrap(),
    );
    orders.update(&mut order1).await?;

    // Create second order with keyboard
    let mut order2 = orders
        .create(NewOrderBuilder::default().id(order_id_2).build().unwrap())
        .await?;
    order2.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id_2)
            .product_name("Keyboard")
            .quantity(1)
            .price(79.99)
            .build()
            .unwrap(),
    );
    orders.update(&mut order2).await?;

    // Create third order with monitor and webcam
    let mut order3 = orders
        .create(NewOrderBuilder::default().id(order_id_3).build().unwrap())
        .await?;
    order3.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id_3)
            .product_name("Monitor")
            .quantity(1)
            .price(299.99)
            .build()
            .unwrap(),
    );
    order3.add_item(
        NewOrderItemBuilder::default()
            .id(OrderItemId::new())
            .order_id(order_id_3)
            .product_name("Webcam")
            .quantity(1)
            .price(89.99)
            .build()
            .unwrap(),
    );
    orders.update(&mut order3).await?;

    // Use find_all to load all orders at once
    let all_order_ids = vec![order_id_1, order_id_2, order_id_3];
    let loaded_orders = orders.find_all::<Order>(&all_order_ids).await?;

    // Verify we got all 3 orders
    assert_eq!(loaded_orders.len(), 3);

    // Verify nested entities were loaded for each order
    let loaded_order_1 = loaded_orders.get(&order_id_1).unwrap();
    assert_eq!(loaded_order_1.n_items(), 2);
    assert!(loaded_order_1.find_item_with_name("Laptop").is_some());
    assert!(loaded_order_1.find_item_with_name("Mouse").is_some());

    let loaded_order_2 = loaded_orders.get(&order_id_2).unwrap();
    assert_eq!(loaded_order_2.n_items(), 1);
    assert!(loaded_order_2.find_item_with_name("Keyboard").is_some());

    let loaded_order_3 = loaded_orders.get(&order_id_3).unwrap();
    assert_eq!(loaded_order_3.n_items(), 2);
    assert!(loaded_order_3.find_item_with_name("Monitor").is_some());
    assert!(loaded_order_3.find_item_with_name("Webcam").is_some());

    // Verify specific item details to ensure nested entities are fully loaded
    let laptop = loaded_order_1.find_item_with_name("Laptop").unwrap();
    assert_eq!(laptop.quantity, 1);
    assert_eq!(laptop.price, 999.99);

    let keyboard = loaded_order_2.find_item_with_name("Keyboard").unwrap();
    assert_eq!(keyboard.quantity, 1);
    assert_eq!(keyboard.price, 79.99);

    let monitor = loaded_order_3.find_item_with_name("Monitor").unwrap();
    assert_eq!(monitor.quantity, 1);
    assert_eq!(monitor.price, 299.99);

    let webcam = loaded_order_3.find_item_with_name("Webcam").unwrap();
    assert_eq!(webcam.quantity, 1);
    assert_eq!(webcam.price, 89.99);

    Ok(())
}
