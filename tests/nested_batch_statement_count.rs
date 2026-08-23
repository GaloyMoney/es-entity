#![cfg(feature = "instrument")]
//! Proves that `update_all_in_op`'s nested-children phase vectorizes across
//! the *whole* parent batch instead of degrading to one child-repo call per
//! parent.
//!
//! Every generated bulk repo fn (`create_all_in_op`, `update_all_mut_in_op`)
//! issues exactly one SQL statement for its whole input batch — that's the
//! entire point of the `UNNEST`-based codegen these functions build (see
//! `es-entity-macros/src/repo/{create_all_fn,update_all_fn}.rs`). So counting
//! how many times a child repo's bulk fn is *called* while updating a batch
//! of parents is an exact proxy for how many SQL statements that phase
//! issues: one span per call, one statement per span. A `tracing_subscriber`
//! `Layer` recording `on_new_span` calls gives us that count directly, per
//! the generated `#[tracing::instrument]` span on each bulk fn (gated behind
//! this crate's own `instrument` feature, hence `#![cfg(feature =
//! "instrument")]` on this whole file).

mod entities;
mod helpers;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

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
struct SpanCounts(Arc<Mutex<HashMap<String, usize>>>);

impl SpanCounts {
    fn count(&self, name: &str) -> usize {
        self.0.lock().unwrap().get(name).copied().unwrap_or(0)
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SpanCounts {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        *self
            .0
            .lock()
            .unwrap()
            .entry(attrs.metadata().name().to_string())
            .or_insert(0) += 1;
    }
}

async fn create_order_with_items(
    orders: &Orders,
    n_items: usize,
) -> anyhow::Result<(OrderId, Vec<OrderItemId>)> {
    let order_id = OrderId::new();
    let mut order = orders
        .create(NewOrderBuilder::default().id(order_id).build().unwrap())
        .await?;

    let mut item_ids = Vec::new();
    for i in 0..n_items {
        let item_id = OrderItemId::new();
        item_ids.push(item_id);
        order.add_item(
            NewOrderItemBuilder::default()
                .id(item_id)
                .order_id(order_id)
                .product_name(format!("item-{i}"))
                .quantity(1)
                .price(9.99)
                .build()
                .unwrap(),
        );
    }
    orders.update(&mut order).await?;

    Ok((order_id, item_ids))
}

/// N parents, each with M already-persisted children being updated and one
/// brand-new child being added in the same `update_all_in_op` call: the
/// persisted-child updates should collapse into a single
/// `order_items.update_all_mut` call, and the new-child creates into a
/// single `order_items.create_all` call — not one of each per parent.
#[tokio::test]
async fn update_all_batches_nested_children_across_parents() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    const N_PARENTS: usize = 5;
    const M_PERSISTED_ITEMS: usize = 2;

    let mut order_ids = Vec::new();
    for _ in 0..N_PARENTS {
        let (order_id, _items) = create_order_with_items(&orders, M_PERSISTED_ITEMS).await?;
        order_ids.push(order_id);
    }

    // Load every order fresh (with its persisted items populated), mutate one
    // persisted item and add one brand-new item per order, then flush every
    // parent through a single `update_all_in_op` call.
    let mut loaded = orders.find_all::<Order>(&order_ids).await?;
    let mut batch: Vec<Order> = order_ids
        .iter()
        .map(|id| loaded.remove(id).expect("order was loaded"))
        .collect();

    for order in batch.iter_mut() {
        order
            .update_item_quantity("item-0", 42)
            .expect("item-0 exists");
        order.add_item(
            NewOrderItemBuilder::default()
                .id(OrderItemId::new())
                .order_id(order.id)
                .product_name("new-item")
                .quantity(1)
                .price(1.23)
                .build()
                .unwrap(),
        );
    }

    let counts = SpanCounts::default();
    let subscriber = tracing_subscriber::registry().with(counts.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    // `Order` itself never gains new events here (only its nested `items`
    // do), so `n_events` — which counts *parent*-level events — is
    // legitimately 0; the nested phase runs unconditionally regardless, and
    // the span counts plus the reload below are what confirm it actually
    // persisted.
    orders.update_all(&mut batch).await?;

    assert_eq!(
        counts.count("order_items.update_all_mut"),
        1,
        "persisted-child updates across all {N_PARENTS} parents should collapse into one \
         order_items.update_all_mut call, not one per parent"
    );
    assert_eq!(
        counts.count("order_items.create_all"),
        1,
        "new-child creates across all {N_PARENTS} parents should collapse into one \
         order_items.create_all call, not one per parent"
    );
    // Sanity: the per-child path this replaces would have been visible as
    // `order_items.update` (singular), once per persisted child touched.
    assert_eq!(
        counts.count("order_items.update"),
        0,
        "the batched path should never fall back to the per-child update_in_op"
    );

    // Confirm the writes actually landed, distributed back to the right
    // parents (not just that the statement count looks right).
    let mut reloaded = orders.find_all::<Order>(&order_ids).await?;
    for order_id in &order_ids {
        let order = reloaded.remove(order_id).expect("order reloaded");
        assert_eq!(order.n_items(), M_PERSISTED_ITEMS + 1, "order {order_id}");
        let item0 = order
            .find_item_with_name("item-0")
            .expect("item-0 still present");
        assert_eq!(item0.quantity, 42, "order {order_id}'s item-0 was updated");
        assert!(
            order.find_item_with_name("new-item").is_some(),
            "order {order_id}'s new item was created"
        );
    }

    Ok(())
}

#[tokio::test]
async fn update_all_mut_accepts_a_chained_iterator() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    const N_PARENTS: usize = 3;
    const M_ITEMS: usize = 2;

    let mut order_ids = Vec::new();
    for _ in 0..N_PARENTS {
        let (order_id, _items) = create_order_with_items(&orders, M_ITEMS).await?;
        order_ids.push(order_id);
    }

    let mut loaded = orders.find_all::<Order>(&order_ids).await?;
    let mut batch: Vec<Order> = order_ids
        .iter()
        .map(|id| loaded.remove(id).expect("order was loaded"))
        .collect();

    for order in batch.iter_mut() {
        order
            .update_item_quantity("item-0", 99)
            .expect("item-0 exists");
    }

    let mut op = orders.begin_op().await?;
    let n_events = orders
        .items
        .update_all_mut_in_op(
            &mut op,
            batch
                .iter_mut()
                .flat_map(|order| order.iter_persisted_children_mut()),
        )
        .await?;
    op.commit().await?;

    assert_eq!(
        n_events, N_PARENTS,
        "one QuantityUpdated event per parent's item-0"
    );

    let mut reloaded = orders.find_all::<Order>(&order_ids).await?;
    for order_id in &order_ids {
        let order = reloaded.remove(order_id).expect("order reloaded");
        let item0 = order
            .find_item_with_name("item-0")
            .expect("item-0 still present");
        assert_eq!(item0.quantity, 99, "order {order_id}'s item-0 was updated");
    }

    Ok(())
}
