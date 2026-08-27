//! Aggregate-version concurrency tests.
//!
//! These prove an optimistic-concurrency property, so they run **two real
//! writers against two real transactions** and order the interleaving with
//! explicit handshakes (`tokio::sync::oneshot` / `Barrier`). Sequential writes
//! cannot demonstrate OCC, and sleep-based ordering passes for the wrong reason
//! and flakes in CI, so neither is used here.
//!
//! The property under test: every write anywhere in a nested aggregate bumps
//! one version on the root, so two writers who each loaded the same root see
//! exactly one winner — even when they touch *different* children, which
//! per-entity `UNIQUE(id, sequence)` OCC can never catch.

mod entities;
mod helpers;

use entities::order::*;
use es_entity::*;
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

fn new_item(order_id: OrderId, name: &str, quantity: i32) -> NewOrderItem {
    NewOrderItemBuilder::default()
        .id(OrderItemId::new())
        .order_id(order_id)
        .product_name(name.to_string())
        .quantity(quantity)
        .price(1.0)
        .build()
        .unwrap()
}

/// Creates an order with two distinct persisted items, so the two writers in
/// the tests below can each mutate a *different* child.
async fn seed_order_with_two_items(orders: &Orders) -> anyhow::Result<OrderId> {
    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();
    let mut order = orders.create(new_order).await?;
    order.add_item(new_item(order_id, "left", 1));
    order.add_item(new_item(order_id, "right", 1));
    orders.update(&mut order).await?;
    Ok(order_id)
}

/// Reads the raw version straight from the index table, bypassing the repo.
async fn raw_version(pool: &PgPool, id: OrderId) -> anyhow::Result<i32> {
    let row = sqlx::query!("SELECT version FROM orders WHERE id = $1", id as OrderId)
        .fetch_one(pool)
        .await?;
    Ok(row.version)
}

// ---------------------------------------------------------------------------
// Write skew — the case per-entity OCC cannot catch
// ---------------------------------------------------------------------------

/// Two writers load the same aggregate and each mutates a **different** child.
///
/// Before aggregate versioning both commits succeeded: neither appended to the
/// parent stream, and the two children have independent sequence chains, so
/// there was no conflict basis anywhere. Now they contend on the root version
/// and exactly one wins.
///
/// Revert-to-red: remove the CAS from `update_fn.rs` and this fails
/// deterministically — both writers return `Ok`, and the `expect_err` below
/// panics on every run rather than intermittently.
#[tokio::test]
async fn concurrent_mutations_of_different_children_conflict() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());
    let order_id = seed_order_with_two_items(&orders).await?;

    // Both writers hydrate from the same committed state, so both hold the
    // same aggregate version.
    let mut order_a = orders.find_by_id(order_id).await?;
    let mut order_b = orders.find_by_id(order_id).await?;

    assert!(order_a.update_item_quantity("left", 10).did_execute());
    assert!(order_b.update_item_quantity("right", 20).did_execute());

    // Two genuinely concurrent transactions. `ready_*` / `go_*` sequence them
    // so writer A's CAS is known to have run before B's is attempted, without
    // any sleeping: B is released only once A has actually written.
    let (ready_a_tx, ready_a_rx) = tokio::sync::oneshot::channel::<()>();
    let (go_b_tx, go_b_rx) = tokio::sync::oneshot::channel::<()>();
    let (a_done_tx, a_done_rx) = tokio::sync::oneshot::channel::<()>();

    let orders_a = Orders::new(pool.clone());
    let writer_a = tokio::spawn(async move {
        let mut op = orders_a.begin_op().await?;
        orders_a.update_in_op(&mut op, &mut order_a).await?;
        // A has written but NOT committed. Hand over to B so B's CAS races a
        // still-open transaction, then commit once B has been released.
        ready_a_tx.send(()).unwrap();
        go_b_rx.await.unwrap();
        op.commit().await?;
        a_done_tx.send(()).unwrap();
        Ok::<_, anyhow::Error>(())
    });

    ready_a_rx.await.unwrap();

    let orders_b = Orders::new(pool.clone());
    let writer_b = tokio::spawn(async move {
        // Release A to commit, then wait for that commit to land before
        // attempting B's CAS. This makes the ordering deterministic: B always
        // sees A's committed version bump.
        go_b_tx.send(()).unwrap();
        a_done_rx.await.unwrap();
        let mut op = orders_b.begin_op().await?;
        let res = orders_b.update_in_op(&mut op, &mut order_b).await;
        Ok::<_, OrderModifyError>(res.err())
    });

    writer_a.await??;
    let b_err = writer_b
        .await?
        .expect("writer B task should not itself fail")
        .expect("writer B must be rejected: it holds a stale aggregate version");

    // Assert the specific error, not merely that something failed.
    assert!(
        b_err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {b_err}"
    );

    // Exactly one bump landed: seed create (1) -> seed update (2) -> A (3).
    assert_eq!(raw_version(&pool, order_id).await?, 3);

    // A's change is present; B's is not.
    let reloaded = orders.find_by_id(order_id).await?;
    assert_eq!(
        reloaded.find_item_with_name("left").unwrap().quantity,
        10,
        "winner's write must be durable"
    );
    assert_eq!(
        reloaded.find_item_with_name("right").unwrap().quantity,
        1,
        "loser's write must not have landed"
    );

    Ok(())
}

/// The creation flavour of write skew: both writers *add* a child.
///
/// This one per-entity OCC could never catch even in principle — a brand-new
/// child has no prior sequence to collide on, so before this change both
/// inserts committed and the aggregate silently ended up with both.
#[tokio::test]
async fn concurrent_child_creation_conflicts() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();
    orders.create(new_order).await?;

    let mut order_a = orders.find_by_id(order_id).await?;
    let mut order_b = orders.find_by_id(order_id).await?;
    order_a.add_item(new_item(order_id, "from-a", 1));
    order_b.add_item(new_item(order_id, "from-b", 1));

    let (a_done_tx, a_done_rx) = tokio::sync::oneshot::channel::<()>();

    let orders_a = Orders::new(pool.clone());
    let writer_a = tokio::spawn(async move {
        orders_a.update(&mut order_a).await?;
        a_done_tx.send(()).unwrap();
        Ok::<_, anyhow::Error>(())
    });

    let orders_b = Orders::new(pool.clone());
    let writer_b = tokio::spawn(async move {
        a_done_rx.await.unwrap();
        Ok::<_, anyhow::Error>(orders_b.update(&mut order_b).await.err())
    });

    writer_a.await??;
    let b_err = writer_b
        .await??
        .expect("second creator must be rejected on a stale aggregate version");
    assert!(
        b_err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {b_err}"
    );

    let reloaded = orders.find_by_id(order_id).await?;
    assert_eq!(
        reloaded.n_items(),
        1,
        "only the winning writer's child may exist"
    );
    assert!(reloaded.find_item_with_name("from-a").is_some());
    assert!(reloaded.find_item_with_name("from-b").is_none());

    Ok(())
}

// ---------------------------------------------------------------------------
// Version bookkeeping
// ---------------------------------------------------------------------------

/// One bump per update regardless of how many children changed, and no bump at
/// all for a genuine no-op.
#[tokio::test]
async fn version_bumps_once_per_update_and_never_for_a_noop() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());
    let order_id = seed_order_with_two_items(&orders).await?;
    let after_seed = raw_version(&pool, order_id).await?;

    // Two children mutated in one update -> exactly one bump.
    let mut order = orders.find_by_id(order_id).await?;
    assert!(order.update_item_quantity("left", 7).did_execute());
    assert!(order.update_item_quantity("right", 8).did_execute());
    orders.update(&mut order).await?;
    assert_eq!(raw_version(&pool, order_id).await?, after_seed + 1);

    // Nothing to write anywhere -> no CAS, no bump, Ok(0).
    let mut untouched = orders.find_by_id(order_id).await?;
    assert_eq!(orders.update(&mut untouched).await?, 0);
    assert_eq!(raw_version(&pool, order_id).await?, after_seed + 1);

    Ok(())
}

/// A no-op update on a **stale** entity still succeeds.
///
/// Ratified carve-out, not an oversight: an update with nothing to write does
/// not touch the database at all, so there is no CAS to fail. Documented
/// behaviour — a test pins it so it is not "fixed" by accident.
#[tokio::test]
async fn noop_update_on_a_stale_entity_is_ok() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());
    let order_id = seed_order_with_two_items(&orders).await?;

    let mut stale = orders.find_by_id(order_id).await?;

    // Move the aggregate underneath `stale`.
    let mut other = orders.find_by_id(order_id).await?;
    assert!(other.update_item_quantity("left", 42).did_execute());
    orders.update(&mut other).await?;
    let version_after = raw_version(&pool, order_id).await?;

    // `stale` has no pending work, so this is a no-op and must not fail.
    assert_eq!(orders.update(&mut stale).await?, 0);
    assert_eq!(raw_version(&pool, order_id).await?, version_after);

    Ok(())
}

/// Create starts the clock at 1 and tracks it in memory, so a freshly created
/// aggregate can be mutated and updated without an intervening reload.
#[tokio::test]
async fn create_starts_at_version_one_and_tracks_in_memory() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();
    let mut order = orders.create(new_order).await?;
    assert_eq!(raw_version(&pool, order_id).await?, 1);

    // No reload between create and update.
    order.add_item(new_item(order_id, "widget", 3));
    orders.update(&mut order).await?;
    assert_eq!(raw_version(&pool, order_id).await?, 2);

    // And again, still without reloading.
    assert!(order.update_item_quantity("widget", 5).did_execute());
    orders.update(&mut order).await?;
    assert_eq!(raw_version(&pool, order_id).await?, 3);

    Ok(())
}

/// An entity that never came from the repo has no version to check, so a root
/// update must refuse rather than bump unguarded.
///
/// Note that an entity moved around by value keeps its version: the version
/// lives on `EntityEvents`, so re-running `try_from_events` over events that
/// *did* come from a repo stays hydrated. Getting a genuinely unhydrated root
/// means going through `IntoEvents` without ever persisting, as below.
#[tokio::test]
async fn update_of_an_unhydrated_root_is_refused() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();

    // Hydrate straight from `into_events` — the entity has never been through
    // `create`, so there is no row and no aggregate version.
    let mut detached = Order::try_from_events(new_order.into_events())?;
    assert_eq!(
        detached.events().aggregate_version(),
        None,
        "an entity that never touched the repo must carry no version"
    );
    detached.add_item(new_item(order_id, "orphan", 1));

    let err = orders
        .update(&mut detached)
        .await
        .expect_err("an unhydrated root must not be updatable");
    assert!(
        matches!(err, OrderModifyError::EntityNotHydrated),
        "expected EntityNotHydrated, got: {err}"
    );

    // And nothing was written.
    assert!(
        sqlx::query!(
            "SELECT version FROM orders WHERE id = $1",
            order_id as OrderId
        )
        .fetch_optional(&pool)
        .await?
        .is_none(),
        "a refused update must not have created a row"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Validation primitive
// ---------------------------------------------------------------------------

/// `validate_aggregate_versions` reports exactly the roots that moved — and
/// only those — out of a mixed batch.
#[tokio::test]
async fn validate_aggregate_versions_isolates_the_stale_root() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let stable = seed_order_with_two_items(&orders).await?;
    let moving = seed_order_with_two_items(&orders).await?;

    let checks = vec![
        (stable, raw_version(&pool, stable).await?),
        (moving, raw_version(&pool, moving).await?),
    ];

    let mut op = orders.begin_op().await?;
    let stale = Orders::validate_aggregate_versions(&mut op, &checks).await?;
    assert!(stale.is_empty(), "nothing has moved yet");
    op.commit().await?;

    // Move exactly one of them.
    let mut order = orders.find_by_id(moving).await?;
    assert!(order.update_item_quantity("left", 77).did_execute());
    orders.update(&mut order).await?;

    let mut op = orders.begin_op().await?;
    let stale = Orders::validate_aggregate_versions(&mut op, &checks).await?;
    op.commit().await?;

    assert_eq!(stale, vec![moving], "only the moved root may be reported");

    Ok(())
}

// ---------------------------------------------------------------------------
// Torn reads
// ---------------------------------------------------------------------------

/// Reads stay internally consistent and do not spuriously fail while the
/// aggregate is being rewritten underneath them.
///
/// **What this does and does not prove.** It exercises the read path under real
/// concurrent writes and pins that a reader never sees a half-written aggregate
/// or an unexpected error. It does *not* on its own prove the version bracket
/// closes Hole 2: with the CAS disabled this test still passes, because the
/// `Order` fixture's parent carries no state of its own and all of a parent's
/// children load in a single statement, so there is nothing here that *can*
/// tear. A test that genuinely goes red without the bracket needs a fixture
/// whose parent holds state derived from its children (or a three-level tree
/// with mutable mid-level state, which `gc_children` currently lacks).
///
/// The read-side machinery is covered instead by
/// `validate_aggregate_versions_isolates_the_stale_root`, which does fail
/// deterministically when the mechanism is removed.
#[tokio::test]
async fn concurrent_reads_stay_consistent_and_do_not_spuriously_fail() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let order_id = OrderId::new();
    let new_order = NewOrderBuilder::default().id(order_id).build().unwrap();
    let mut order = orders.create(new_order).await?;
    order.add_item(new_item(order_id, "a", 1));
    order.add_item(new_item(order_id, "b", 1));
    orders.update(&mut order).await?;

    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

    let writer_orders = Orders::new(pool.clone());
    let writer = tokio::spawn(async move {
        for i in 0..40 {
            let mut order = writer_orders.find_by_id(order_id).await?;
            // Mutate both children together, so any reader that saw one
            // without the other would be reading a torn aggregate.
            let _ = order.update_item_quantity("a", i + 2);
            let _ = order.update_item_quantity("b", i + 2);
            match writer_orders.update(&mut order).await {
                Ok(_) => {}
                // The reader never writes, so contention here is not expected,
                // but a lost race is not a test failure either.
                Err(e) if e.was_concurrent_modification() => continue,
                Err(e) => return Err(e.into()),
            }
        }
        stop_tx.send(()).unwrap();
        Ok::<_, anyhow::Error>(())
    });

    let reader_orders = Orders::new(pool.clone());
    let reader = tokio::spawn(async move {
        let mut reads = 0usize;
        loop {
            match reader_orders.find_by_id(order_id).await {
                Ok(order) => {
                    // Both children are always present and always carry the
                    // same quantity, because the writer only ever sets them
                    // together within one aggregate write (and they load in one
                    // statement, so this holds with or without the bracket —
                    // see the caveat on this test).
                    assert_eq!(order.n_items(), 2, "aggregate must never appear torn");
                    let a = order.find_item_with_name("a").unwrap().quantity;
                    let b = order.find_item_with_name("b").unwrap().quantity;
                    assert_eq!(
                        a, b,
                        "parent and children must come from one consistent snapshot"
                    );
                    reads += 1;
                }
                // A read that raced a commit is reported, not silently wrong —
                // that is the whole point of the version bracket.
                Err(e) if e.was_stale_aggregate_read() => {}
                Err(e) => return Err(anyhow::Error::from(e)),
            }
            if stop_rx.try_recv().is_ok() {
                break;
            }
        }
        Ok::<_, anyhow::Error>(reads)
    });

    writer.await??;
    let reads = reader.await??;
    assert!(reads > 0, "reader must have observed at least one state");

    Ok(())
}

// ---------------------------------------------------------------------------
// Grandchildren — the recursive pending-work check
// ---------------------------------------------------------------------------
// Covered in `tests/nested_grandchildren.rs`, which owns the gc_* fixtures.

// ---------------------------------------------------------------------------
// Batch and delete paths
// ---------------------------------------------------------------------------

/// `update_all` on roots CAS-bumps every root it writes.
///
/// This path matters more than it looks: `update_all` runs the nested phase, so
/// without a CAS it would write children with no version bump at all — leaving
/// write skew open here *and* making the read side unsound, since a reader
/// re-checking an unchanged version would wrongly conclude its tree was
/// consistent.
///
/// Revert-to-red: drop the batch CAS and both the version assertion and the
/// stale-writer rejection below fail deterministically.
#[tokio::test]
async fn update_all_cas_bumps_every_root_and_rejects_a_stale_one() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let first = seed_order_with_two_items(&orders).await?;
    let second = seed_order_with_two_items(&orders).await?;
    let v_first = raw_version(&pool, first).await?;
    let v_second = raw_version(&pool, second).await?;

    let mut batch = vec![
        orders.find_by_id(first).await?,
        orders.find_by_id(second).await?,
    ];
    for order in batch.iter_mut() {
        assert!(order.update_item_quantity("left", 31).did_execute());
    }
    orders.update_all(&mut batch).await?;

    assert_eq!(raw_version(&pool, first).await?, v_first + 1);
    assert_eq!(raw_version(&pool, second).await?, v_second + 1);

    // A batch containing one stale root fails as a whole — a partially applied
    // batch is not something callers could reason about.
    let mut stale_batch = vec![orders.find_by_id(first).await?];
    let mut winner = orders.find_by_id(first).await?;
    assert!(winner.update_item_quantity("right", 44).did_execute());
    orders.update(&mut winner).await?;

    for order in stale_batch.iter_mut() {
        assert!(order.update_item_quantity("right", 55).did_execute());
    }
    let err = orders
        .update_all(&mut stale_batch)
        .await
        .expect_err("a batch holding a stale root must be rejected");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    // The loser's write did not land.
    let reloaded = orders.find_by_id(first).await?;
    assert_eq!(reloaded.find_item_with_name("right").unwrap().quantity, 44);

    Ok(())
}

/// An untouched root inside a batch must not have its version bumped — the same
/// no-op carve-out the single-entity path has.
#[tokio::test]
async fn update_all_leaves_untouched_roots_alone() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let touched = seed_order_with_two_items(&orders).await?;
    let untouched = seed_order_with_two_items(&orders).await?;
    let v_untouched = raw_version(&pool, untouched).await?;
    let v_touched = raw_version(&pool, touched).await?;

    let mut batch = vec![
        orders.find_by_id(touched).await?,
        orders.find_by_id(untouched).await?,
    ];
    // Mutate only the first.
    assert!(batch[0].update_item_quantity("left", 61).did_execute());
    orders.update_all(&mut batch).await?;

    assert_eq!(raw_version(&pool, touched).await?, v_touched + 1);
    assert_eq!(
        raw_version(&pool, untouched).await?,
        v_untouched,
        "a root with nothing to write must not bump"
    );

    Ok(())
}

/// Deleting a root is an aggregate write and CASes like any other, so a writer
/// holding a stale version loses the race.
#[tokio::test]
async fn delete_cas_rejects_a_stale_writer() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());
    let order_id = seed_order_with_two_items(&orders).await?;

    let stale = orders.find_by_id(order_id).await?;

    // Move the aggregate underneath the stale handle.
    let mut winner = orders.find_by_id(order_id).await?;
    assert!(winner.update_item_quantity("left", 71).did_execute());
    orders.update(&mut winner).await?;

    let err = orders
        .delete(stale)
        .await
        .expect_err("deleting on a stale version must be rejected");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    // Still there, and still holding the winner's write.
    let reloaded = orders.find_by_id(order_id).await?;
    assert_eq!(reloaded.find_item_with_name("left").unwrap().quantity, 71);

    // A fresh handle deletes fine and bumps the version.
    let fresh = orders.find_by_id(order_id).await?;
    let before = raw_version(&pool, order_id).await?;
    orders.delete(fresh).await?;
    assert_eq!(raw_version(&pool, order_id).await?, before + 1);

    Ok(())
}

/// `update_all_mut_in_op` is the other batch shape (`RefVec`): it takes
/// scattered `&mut` borrows rather than a contiguous slice, and it is public on
/// root repos too. It gets its own CAS codegen, so it needs its own coverage —
/// `update_all` above only exercises the `OwnedSlice` shape.
#[tokio::test]
async fn update_all_mut_in_op_cas_covers_the_ref_batch_shape() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let orders = Orders::new(pool.clone());

    let first = seed_order_with_two_items(&orders).await?;
    let second = seed_order_with_two_items(&orders).await?;
    let v_first = raw_version(&pool, first).await?;
    let v_second = raw_version(&pool, second).await?;

    let mut a = orders.find_by_id(first).await?;
    let mut b = orders.find_by_id(second).await?;
    assert!(a.update_item_quantity("left", 81).did_execute());
    assert!(b.update_item_quantity("left", 82).did_execute());

    let mut op = orders.begin_op().await?;
    orders
        .update_all_mut_in_op(&mut op, vec![&mut a, &mut b])
        .await?;
    op.commit().await?;

    assert_eq!(raw_version(&pool, first).await?, v_first + 1);
    assert_eq!(raw_version(&pool, second).await?, v_second + 1);

    // And a stale member is rejected on this shape too.
    let mut stale = orders.find_by_id(first).await?;
    let mut winner = orders.find_by_id(first).await?;
    assert!(winner.update_item_quantity("right", 83).did_execute());
    orders.update(&mut winner).await?;

    assert!(stale.update_item_quantity("right", 84).did_execute());
    let mut op = orders.begin_op().await?;
    let err = orders
        .update_all_mut_in_op(&mut op, vec![&mut stale])
        .await
        .expect_err("a stale root must be rejected on the ref-batch shape too");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// List paging — the peeked-but-unreturned entity
// ---------------------------------------------------------------------------
//
// `list_by_id` (and any other paginated read) over-fetches by one entity to
// compute `has_next_page`. That extra entity's aggregate version must not be
// validated against the *current* page — it was never handed back to the
// caller, so a concurrent write to it proves nothing about staleness here.
//
// Dedicated `paging_orders`/`paging_order_items` tables, NOT the shared
// `orders`/`order_items` used above: this test pages the whole table with no
// filter, and those tables are touched unscoped by every other test in every
// file that imports `entities::order` — sharing them would make this flaky
// for reasons unrelated to the bug it proves.
//
// This also reuses `entities::order::Order` as its entity, so it needs its
// own module: `#[derive(EsRepo)]` generates `OrderQueryError` and friends at
// the scope it's invoked in, and `Orders` above already owns those names at
// this file's top level.
mod list_paging {
    // Not `use super::*`: the parent module's `Orders`/`OrderItems` derives
    // (also `entity = "Order"` / `"OrderItem"`) already generated sibling
    // modules named after the entity (`order_cursor`, `order_repo_types`,
    // ...) at the parent scope. A glob import would pull those in and
    // collide with the identically-named modules this file's own derives
    // below generate.
    use crate::{entities::order::*, helpers, new_item};
    use es_entity::*;
    use sqlx::PgPool;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "Order",
        tbl = "paging_orders",
        events_tbl = "paging_order_events"
    )]
    pub struct PagingOrders {
        pool: PgPool,

        #[es_repo(nested)]
        items: PagingOrderItems,
    }

    impl PagingOrders {
        pub fn new(pool: PgPool) -> Self {
            Self {
                pool: pool.clone(),
                items: PagingOrderItems::new(pool),
            }
        }
    }

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "OrderItem",
        tbl = "paging_order_items",
        events_tbl = "paging_order_item_events",
        columns(order_id(ty = "OrderId", update(persist = false), parent))
    )]
    pub struct PagingOrderItems {
        // Only ever reached through `PagingOrders`' generated nested-child calls,
        // which resolve by type (`<PagingOrderItems>::populate_in_op`, etc.), not
        // through an instance of this struct — this file never calls a
        // `PagingOrderItems` method directly, so the field itself is unread.
        #[allow(dead_code)]
        pool: PgPool,
    }

    impl PagingOrderItems {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }
    }

    /// Bugbot finding: `fetch_n_inner` captured an aggregate version for every
    /// row the underlying query returned — including the extra entity read
    /// solely to decide `has_next_page` — before `EntityEvents::load_n` truncated
    /// down to the requested page. A concurrent write to that unreturned entity
    /// therefore failed the *current* page with `StaleAggregateRead`, even though
    /// the page never contained it.
    ///
    /// Two roots, `kept` (the one a `first: 1` ascending page returns) and
    /// `peeked` (the one fetched only to prove `has_next_page = true`). A writer
    /// hammers `peeked` continuously; the reader pages with `first: 1` in a tight
    /// loop. `peeked`'s id sorts after `kept`'s by construction, so it can never
    /// legitimately be the entity returned — any error here can only come from
    /// `peeked`'s version riding along into the validation it was excluded from.
    ///
    /// Revert-to-red: remove `versions.truncate(entities.len())` in
    /// `fetch_n_inner` (src/query.rs) and this fails — not on every iteration,
    /// since it needs the writer's commit to land inside a specific window
    /// relative to the reader's own SELECT and validate statements, but reliably
    /// within the loop below.
    #[tokio::test]
    async fn list_page_does_not_fail_on_a_peeked_but_unreturned_entity() -> anyhow::Result<()> {
        let pool = helpers::init_pool().await?;
        let orders = PagingOrders::new(pool.clone());

        // `paging_orders` is dedicated to this one test, but nothing else clears
        // it between runs — a re-run against the same dev DB would otherwise
        // accumulate rows from every prior run, and `list_by_id` pages the whole
        // table unscoped, so an old row could out-sort `kept` and break the
        // "the returned page is exactly `kept`" assumption below.
        sqlx::query!("DELETE FROM paging_order_item_events")
            .execute(&pool)
            .await?;
        sqlx::query!("DELETE FROM paging_order_items")
            .execute(&pool)
            .await?;
        sqlx::query!("DELETE FROM paging_order_events")
            .execute(&pool)
            .await?;
        sqlx::query!("DELETE FROM paging_orders")
            .execute(&pool)
            .await?;

        let new_order = |id| NewOrderBuilder::default().id(id).build().unwrap();
        let a = orders.create(new_order(OrderId::new())).await?.id;
        let b = orders.create(new_order(OrderId::new())).await?.id;
        let (kept, peeked) = if a < b { (a, b) } else { (b, a) };

        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

        let writer_orders = PagingOrders::new(pool.clone());
        let writer = tokio::spawn(async move {
            for i in 0..300 {
                let mut order = writer_orders.find_by_id(peeked).await?;
                order.add_item(new_item(peeked, "x", i));
                match writer_orders.update(&mut order).await {
                    Ok(_) => {}
                    // The reader never writes, so contention here is not
                    // expected, but a lost race is not a test failure either.
                    Err(e) if e.was_concurrent_modification() => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            // A dropped receiver means the reader already exited (via `?`, on
            // the very error this test is trying to catch) — that is reported
            // through `reader.await??` below, not by panicking here on send.
            let _ = stop_tx.send(());
            Ok::<_, anyhow::Error>(())
        });

        let reader_orders = PagingOrders::new(pool.clone());
        let reader = tokio::spawn(async move {
            let mut reads = 0usize;
            loop {
                let page = reader_orders
                    .list_by_id(
                        es_entity::PaginatedQueryArgs {
                            first: 1,
                            after: None,
                        },
                        es_entity::ListDirection::Ascending,
                    )
                    .await?;
                assert_eq!(
                    page.entities.len(),
                    1,
                    "exactly one root exists at `kept`'s position"
                );
                assert_eq!(
                    page.entities[0].id, kept,
                    "the concurrently-written root must never be the one returned"
                );
                assert!(
                    page.has_next_page,
                    "`peeked` must still be there, just unreturned"
                );
                reads += 1;
                if stop_rx.try_recv().is_ok() {
                    break;
                }
            }
            Ok::<_, anyhow::Error>(reads)
        });

        writer.await??;
        let reads = reader.await??;
        assert!(reads > 0, "reader must have observed at least one page");

        Ok(())
    }
} // mod list_paging
