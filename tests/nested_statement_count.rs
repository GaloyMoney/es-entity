#![cfg(feature = "instrument")]
//! Proves the core deliverable of single-statement nested reads (see
//! `es-entity-dev/spec-single-query-nested-reads.md`): every generated read
//! on a nested repo fetches the entire tree in exactly one SQL statement,
//! from a plain `&pool` — no transaction anywhere.
//!
//! Counts `tracing` events on the `sqlx::query` target: sqlx emits exactly
//! one such event per executed statement, independent of es-entity's own
//! `instrument` feature (that gates es-entity's *own* spans, not sqlx's —
//! but `tracing` itself is only pulled in as a dependency behind this
//! feature, hence gating the whole file on it, mirroring
//! `nested_batch_statement_count.rs`).

mod entities;
mod helpers;

use std::sync::{Arc, Mutex};

use entities::order::*;
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;
use tracing_subscriber::layer::SubscriberExt;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Order", delete = "soft")]
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
    delete = "soft",
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

#[derive(Clone, Default)]
struct QueryEventCount(Arc<Mutex<usize>>);

impl QueryEventCount {
    fn get(&self) -> usize {
        *self.0.lock().unwrap()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for QueryEventCount {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() == "sqlx::query" {
            *self.0.lock().unwrap() += 1;
        }
    }
}

async fn seed_order(orders: &Orders, item_names: &[&str]) -> anyhow::Result<OrderId> {
    let order_id = OrderId::new();
    let mut order = orders
        .create(NewOrderBuilder::default().id(order_id).build().unwrap())
        .await?;
    for name in item_names {
        order.add_item(
            NewOrderItemBuilder::default()
                .id(OrderItemId::new())
                .order_id(order_id)
                .product_name(*name)
                .quantity(1)
                .price(1.0)
                .build()
                .unwrap(),
        );
    }
    orders.update(&mut order).await?;
    Ok(order_id)
}

/// A nested `find_by_id` reads its whole tree (parent + children) in exactly
/// one SQL statement, from a plain `&pool` — no `begin_op()`/transaction
/// anywhere. This is the core deliverable: nested reads no longer require
/// `AtomicOperation`.
#[tokio::test]
async fn nested_find_by_id_is_one_statement_no_transaction() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    let order_id = seed_order(&orders, &["Laptop", "Mouse", "Keyboard"]).await?;

    let counter = QueryEventCount::default();
    let subscriber = tracing_subscriber::registry().with(counter.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    // `&orders` only — the generated `find_by_id` takes `impl
    // IntoOneTimeExecutor` now, so this compiles and runs without ever
    // calling `begin_op()`.
    let order = orders.find_by_id(order_id).await?;

    assert_eq!(
        counter.get(),
        1,
        "a nested find_by_id must issue exactly one SQL statement for the whole tree"
    );
    assert_eq!(order.n_items(), 3);

    Ok(())
}

/// `find_all` on N ids, each with children, is also one statement, not one
/// per id and not one per level.
#[tokio::test]
async fn nested_find_all_is_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    let mut ids = Vec::new();
    for i in 0..5 {
        ids.push(seed_order(&orders, &[&format!("item-{i}-a"), &format!("item-{i}-b")]).await?);
    }

    let counter = QueryEventCount::default();
    let subscriber = tracing_subscriber::registry().with(counter.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let loaded = orders.find_all::<Order>(&ids).await?;

    assert_eq!(
        counter.get(),
        1,
        "find_all across 5 parents with children must still be one statement"
    );
    assert_eq!(loaded.len(), 5);
    for order in loaded.values() {
        assert_eq!(order.n_items(), 2);
    }

    Ok(())
}

/// `list_by_id` (cursor pagination) is likewise one statement, covering the
/// `LIMIT`-bearing path, not just point reads.
#[tokio::test]
async fn nested_list_by_id_is_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    for i in 0..3 {
        seed_order(&orders, &[&format!("item-{i}")]).await?;
    }

    let counter = QueryEventCount::default();
    let subscriber = tracing_subscriber::registry().with(counter.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let page = orders
        .list_by_id(
            es_entity::PaginatedQueryArgs {
                first: 10,
                after: None,
            },
            es_entity::ListDirection::Ascending,
        )
        .await?;

    assert_eq!(
        counter.get(),
        1,
        "list_by_id must issue exactly one SQL statement regardless of page size"
    );
    assert!(page.entities.len() >= 3);

    Ok(())
}
