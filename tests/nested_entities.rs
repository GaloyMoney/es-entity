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
    columns(order_id(ty = "OrderId", update(persist = false), list_for, parent))
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
