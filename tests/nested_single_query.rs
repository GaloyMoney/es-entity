//! Correctness properties specific to single-statement nested reads (see
//! `es-entity-dev/spec-single-query-nested-reads.md`):
//!
//! - `LIMIT`/pagination bounds root entities, not joined rows;
//! - per-entity event ordering survives an interleaving that would break a
//!   naive "just concatenate the branches" implementation.
//!
//! (The statement-count=1 proof lives in `tests/nested_statement_count.rs`,
//! gated behind the `instrument` feature — it needs the optional `tracing`
//! dependency that feature pulls in.)

mod entities;
mod helpers;

use entities::order::*;
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;

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

    /// Ad hoc scoped-and-bounded query for the LIMIT test below: bounds to a
    /// known, closed set of ids (immune to other tests' rows sharing the
    /// same physical table) *and* exercises the real `LIMIT`/peek-ahead path
    /// nested list fns use, via `es_query!` directly.
    async fn first_n_of(
        &self,
        ids: &[OrderId],
        first: usize,
    ) -> Result<(Vec<Order>, bool), OrderQueryError> {
        es_entity::es_query!(
            entity = Order,
            "SELECT id FROM orders WHERE id = ANY($2) ORDER BY id LIMIT $1",
            (first + 1) as i64,
            ids as &[OrderId],
        )
        .fetch_n(self.pool(), first)
        .await
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

/// The classic trap: `LIMIT N` over a parent-joined-to-children read must
/// return N *parents*, not N rows. Three parents with deliberately uneven
/// item counts — if `LIMIT` were applied to the joined result instead of a
/// parent-id subquery ahead of the join, a small `first` could return fewer
/// than `first` parents, or a parent with a truncated item set, depending on
/// how many child rows the early parents happen to contribute.
#[tokio::test]
async fn limit_bounds_parents_not_joined_rows() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    let mut expected_items = std::collections::HashMap::new();
    let mut ids = Vec::new();
    for (name_prefix, n) in [("a", 5usize), ("b", 1), ("c", 2)] {
        let names: Vec<String> = (0..n).map(|i| format!("{name_prefix}{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let id = seed_order(&orders, &names).await?;
        expected_items.insert(id, n);
        ids.push(id);
    }

    // Bound to exactly these 3 ids and ask for 2 — the trap fires if LIMIT
    // is applied after the join: the 3rd order's 2 item rows would either
    // truncate the page below 2 parents or cut a parent's own item set short.
    let (page, has_more) = orders.first_n_of(&ids, 2).await?;

    assert_eq!(
        page.len(),
        2,
        "LIMIT must bound the number of parent entities, not the number of joined rows"
    );
    assert!(
        has_more,
        "a 3rd order exists beyond the requested page of 2"
    );
    for order in &page {
        let expected = expected_items
            .get(&order.id)
            .expect("returned order was one of the seeded ones");
        assert_eq!(
            order.n_items(),
            *expected,
            "order {} must carry its FULL item set, not a fragment truncated by LIMIT",
            order.id
        );
    }

    Ok(())
}

/// Event ordering under hydration, via the normal write API: each item's own
/// event stream is only correct if `sequence` is honored per-entity (an
/// entity's `TryFromEvents` folds `QuantityUpdated` by simply overwriting, so
/// out-of-sequence replay silently produces a stale-but-plausible quantity —
/// exactly the kind of bug that survives casual testing).
#[tokio::test]
async fn event_ordering_survives_interleaved_child_writes() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    let order_id = seed_order(&orders, &["z-item", "a-item"]).await?;

    let mut order = orders.find_by_id(order_id).await?;
    order.update_item_quantity("a-item", 10).unwrap();
    order.update_item_quantity("z-item", 20).unwrap();
    order.update_item_quantity("a-item", 30).unwrap();
    orders.update(&mut order).await?;

    let reloaded = orders.find_by_id(order_id).await?;
    let z = reloaded.find_item_with_name("z-item").unwrap();
    let a = reloaded.find_item_with_name("a-item").unwrap();
    assert_eq!(z.quantity, 20, "z-item's single update must apply");
    assert_eq!(
        a.quantity, 30,
        "a-item must reflect its LAST update (10 then 30), not an out-of-sequence intermediate \
         value — this only holds if per-entity `sequence` ordering survived the union"
    );

    Ok(())
}

/// Adversarial version of the above: bypass the write API and INSERT two
/// children's `QuantityUpdated` events directly, in an order that inverts
/// BOTH cross-entity and intra-entity physical row order relative to what a
/// correct hydration must produce. A naive implementation that concatenated
/// the union branches by physical/insertion order instead of the query's
/// explicit `ORDER BY tag, __ord, entity_id, sequence` would misgroup or
/// misorder these rows and this test would fail deterministically.
#[tokio::test]
async fn event_ordering_survives_physically_scrambled_rows() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let orders = Orders::new(pool);

    let order_id = seed_order(&orders, &["item-a", "item-b"]).await?;
    let order = orders.find_by_id(order_id).await?;
    let item_a_id = order.find_item_with_name("item-a").unwrap().id;
    let item_b_id = order.find_item_with_name("item-b").unwrap().id;

    // Physical insertion order: b's sequence 3 lands in the table BEFORE
    // a's sequence 2, which lands BEFORE b's own sequence 2 — both
    // entities' rows interleaved and b's own rows reversed. Only the
    // query's own ORDER BY can recover the correct final state.
    let inserts = [
        (item_b_id, 3i32, 99), // b: final value, inserted first
        (item_a_id, 2i32, 42), // a: only update
        (item_b_id, 2i32, 50), // b: superseded value, inserted last
    ];
    for (id, sequence, quantity) in inserts {
        sqlx::query!(
            r#"INSERT INTO order_item_events (id, sequence, event_type, event, recorded_at)
               VALUES ($1, $2, 'quantity_updated', $3, NOW())"#,
            id as OrderItemId,
            sequence,
            serde_json::json!({ "type": "quantity_updated", "quantity": quantity }),
        )
        .execute(orders.pool())
        .await?;
    }

    let reloaded = orders.find_by_id(order_id).await?;
    let a = reloaded.find_item_with_name("item-a").unwrap();
    let b = reloaded.find_item_with_name("item-b").unwrap();
    assert_eq!(
        a.quantity, 42,
        "item-a's single scrambled-insert update must apply"
    );
    assert_eq!(
        b.quantity, 99,
        "item-b must reflect sequence 3 (99), not sequence 2 (50) — correct only if hydration \
         ordered by `sequence` rather than physical insertion order"
    );

    Ok(())
}
