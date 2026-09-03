//! Live-PG coverage for [`BatchIsolation`] — per-item and bisected failure
//! isolation over real savepoints, plus the caller-driven commit-per-probe
//! shape that drives [`BisectSearch`] directly.
//!
//! The search algorithm has its own unit tests in
//! `src/operation/batch/search.rs`. What Postgres adds here: that a failed
//! probe leaves the transaction usable, that a rolled-back probe's hooks are
//! dropped, and that a closure borrowing `&self` composes inside an
//! `#[async_trait]` runner.
//!
//! Each test owns a disjoint slice of `batch_items.v` so they can run in
//! parallel against one database.

mod helpers;

use std::sync::{Arc, Mutex};

use es_entity::operation::{
    AtomicOperation, BatchIsolation, BisectBudget, BisectSearch, DbOp, ItemOutcome, ProbeVerdict,
    SavepointOp,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};

async fn insert(op: &mut impl AtomicOperation, v: i32) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO batch_items (v) VALUES ($1)")
        .bind(v)
        .execute(op.as_executor())
        .await?;
    Ok(())
}

/// Clears this test's slice of the table so a re-run starts from the same
/// state as a first run.
async fn reset(pool: &sqlx::PgPool, base: i32) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM batch_items WHERE v >= $1 AND v < $2")
        .bind(base)
        .bind(base + 100)
        .execute(pool)
        .await?;
    Ok(())
}

/// Pre-inserts `v`, so that a later insert of the same value raises a real
/// primary-key violation.
async fn seed(pool: &sqlx::PgPool, v: i32) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO batch_items (v) VALUES ($1)")
        .bind(v)
        .execute(pool)
        .await?;
    Ok(())
}

async fn present(pool: &sqlx::PgPool, range: std::ops::Range<i32>) -> anyhow::Result<Vec<i32>> {
    let rows: Vec<(i32,)> =
        sqlx::query_as("SELECT v FROM batch_items WHERE v >= $1 AND v < $2 ORDER BY v")
            .bind(range.start)
            .bind(range.end)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(v,)| v).collect())
}

#[tokio::test]
async fn run_isolated_keeps_batch_mates_committing_after_a_per_item_failure() -> anyhow::Result<()>
{
    const BASE: i32 = 1_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;
    seed(&pool, BASE + 2).await?; // index 2 is the culprit

    let items: Vec<i32> = (0..5).map(|i| BASE + i).collect();
    let mut op = DbOp::init(&pool).await?;

    let outcomes = op
        .run_isolated(&items, async |sp, item| insert(sp, *item).await)
        .await?;

    assert_eq!(outcomes.len(), 5);
    for (idx, outcome) in outcomes.iter().enumerate() {
        if idx == 2 {
            assert!(outcome.is_err(), "index 2 should have failed");
        } else {
            assert!(outcome.is_ok(), "index {idx} should have survived");
        }
    }

    // The transaction is still usable, so its batch-mates commit.
    op.commit().await?;
    assert_eq!(
        present(&pool, BASE..BASE + 5).await?,
        vec![BASE, BASE + 1, BASE + 2, BASE + 3, BASE + 4],
    );

    Ok(())
}

#[tokio::test]
async fn a_clean_bisect_probes_exactly_once() -> anyhow::Result<()> {
    const BASE: i32 = 2_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;

    let items: Vec<i32> = (0..8).map(|i| BASE + i).collect();
    let mut op = DbOp::init(&pool).await?;

    let outcomes = op
        .run_bisected(&items, BisectBudget::Auto, async |sp, slice| {
            for item in slice {
                insert(sp, *item).await?;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await?;

    assert_eq!(outcomes.probes_used, 1, "a clean batch must not bisect");
    assert!(
        outcomes
            .items
            .iter()
            .all(|o| matches!(o, ItemOutcome::Complete))
    );

    op.commit().await?;
    assert_eq!(present(&pool, BASE..BASE + 8).await?.len(), 8);

    Ok(())
}

#[tokio::test]
async fn a_bisect_isolates_its_culprit_and_salvages_the_siblings() -> anyhow::Result<()> {
    const BASE: i32 = 3_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;
    seed(&pool, BASE + 3).await?;

    let items: Vec<i32> = (0..8).map(|i| BASE + i).collect();
    let mut op = DbOp::init(&pool).await?;

    let outcomes = op
        .run_bisected(&items, BisectBudget::Auto, async |sp, slice| {
            for item in slice {
                insert(sp, *item).await?;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await?;

    assert!(
        outcomes.probes_used > 1,
        "a failing batch must have bisected"
    );
    for (idx, outcome) in outcomes.items.iter().enumerate() {
        if idx == 3 {
            assert!(
                matches!(outcome, ItemOutcome::Failed(_)),
                "index 3 should carry its own attributable error, got {outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, ItemOutcome::Complete),
                "index {idx} should have been salvaged, got {outcome:?}"
            );
        }
    }

    op.commit().await?;
    // Every clean sibling landed; the culprit's value is present only from the
    // seed, so the row count is still 8.
    assert_eq!(present(&pool, BASE..BASE + 8).await?.len(), 8);

    Ok(())
}

#[tokio::test]
async fn budget_exhaustion_leaves_unprobed_items_unresolved() -> anyhow::Result<()> {
    const BASE: i32 = 4_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;
    seed(&pool, BASE).await?;

    let items: Vec<i32> = (0..6).map(|i| BASE + i).collect();
    let mut op = DbOp::init(&pool).await?;

    let outcomes = op
        .run_bisected(&items, BisectBudget::MaxProbes(1), async |sp, slice| {
            for item in slice {
                insert(sp, *item).await?;
            }
            Ok::<_, sqlx::Error>(())
        })
        .await?;

    assert_eq!(outcomes.probes_used, 1);
    assert_eq!(outcomes.items.len(), 6, "every input gets one outcome");
    assert!(
        outcomes
            .items
            .iter()
            .all(|o| matches!(o, ItemOutcome::Unresolved))
    );
    assert!(
        outcomes.last_error.is_some(),
        "the caller needs the batch-level error to explain the unresolved items"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Hooks staged inside a probe follow that probe's own fate.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Probe {
    pre: Arc<Mutex<Vec<i32>>>,
    post: Arc<Mutex<Vec<i32>>>,
    rolled_back: Arc<Mutex<Vec<i32>>>,
}

#[derive(Debug)]
struct ProbeHook {
    items: Vec<i32>,
    probe: Probe,
}

impl CommitHook for ProbeHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.probe.pre.lock().unwrap().extend(self.items.clone());
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.probe.post.lock().unwrap().extend(self.items);
    }

    fn on_rollback(self) {
        self.probe.rolled_back.lock().unwrap().extend(self.items);
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.items.append(&mut other.items);
        true
    }
}

#[tokio::test]
async fn a_rolled_back_probe_contributes_no_hook_state() -> anyhow::Result<()> {
    const BASE: i32 = 5_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;
    seed(&pool, BASE + 1).await?;

    let probe = Probe::default();
    let items: Vec<i32> = (0..3).map(|i| BASE + i).collect();
    let mut op = DbOp::init(&pool).await?;

    let outcomes = op
        .run_isolated(&items, async |sp, item| {
            // Registered before the statement that may fail, matching where a
            // repository registers its own post-persist hook.
            let _ = sp.add_commit_hook(ProbeHook {
                items: vec![*item],
                probe: probe.clone(),
            });
            insert(sp, *item).await
        })
        .await?;

    assert!(outcomes[1].is_err());
    // Nothing fires at a savepoint boundary, rolled back or not.
    assert!(probe.pre.lock().unwrap().is_empty());
    assert!(probe.post.lock().unwrap().is_empty());

    op.commit().await?;

    // The surviving items' hooks folded outward and ran once, merged. The
    // rolled-back item contributed nothing, and `on_rollback` stays reserved
    // for a failed commit.
    assert_eq!(*probe.pre.lock().unwrap(), vec![BASE, BASE + 2]);
    assert_eq!(*probe.post.lock().unwrap(), vec![BASE, BASE + 2]);
    assert!(probe.rolled_back.lock().unwrap().is_empty());

    Ok(())
}

// ---------------------------------------------------------------------------
// The caller owns the transaction boundary, committing each clean range.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_caller_driven_search_commits_each_clean_range_as_it_lands() -> anyhow::Result<()> {
    const BASE: i32 = 6_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;
    seed(&pool, BASE + 3).await?;

    let items: Vec<i32> = (0..8).map(|i| BASE + i).collect();
    let mut search: BisectSearch<String> =
        BisectSearch::new(items.len(), BisectBudget::FullResolution);
    let mut committed: Vec<std::ops::Range<usize>> = Vec::new();

    while let Some(range) = search.next_range() {
        // A fresh transaction per probe, committed once the range is clean.
        let mut op = DbOp::init(&pool).await?;

        let mut failure = None;
        for item in &items[range.clone()] {
            if let Err(error) = insert(&mut op, *item).await {
                failure = Some(error.to_string());
                break;
            }
        }

        match failure {
            None => {
                op.commit().await?;
                committed.push(range.clone());
                search.report(range, ProbeVerdict::Clean)?;
            }
            Some(error) => {
                drop(op); // rolls back
                search.report(range, ProbeVerdict::Failed(error))?;
            }
        }
    }

    let outcomes = search.into_outcomes();
    for (idx, outcome) in outcomes.items.iter().enumerate() {
        if idx == 3 {
            assert!(matches!(outcome, ItemOutcome::Failed(_)));
        } else {
            assert_eq!(outcome, &ItemOutcome::Complete, "index {idx}");
        }
    }

    assert!(
        !committed.is_empty(),
        "clean ranges must have committed while the search was still running"
    );
    // Each clean range was made durable by its own commit.
    assert_eq!(present(&pool, BASE..BASE + 8).await?.len(), 8);

    Ok(())
}

// ---------------------------------------------------------------------------
// A closure borrowing `&self`, inside `#[async_trait]`, with a caller-derived
// wrapper context, spawned onto the runtime.
// ---------------------------------------------------------------------------

struct Ctx<'op, 'parent> {
    op: &'op mut SavepointOp<'parent>,
    label: String,
}

es_entity::delegate_atomic_operation!([<'op, 'parent>] Ctx<'op, 'parent>, { s => s.op });

struct Runner {
    offset: i32,
}

impl Runner {
    async fn insert_one(
        &self,
        op: &mut impl AtomicOperation,
        item: &i32,
    ) -> Result<(), sqlx::Error> {
        insert(op, item + self.offset).await
    }
}

#[async_trait::async_trait]
trait BatchRunner: Send + Sync {
    async fn run(&self, op: &mut DbOp<'static>, items: &[i32]) -> Result<usize, sqlx::Error>;
}

#[async_trait::async_trait]
impl BatchRunner for Runner {
    async fn run(&self, op: &mut DbOp<'static>, items: &[i32]) -> Result<usize, sqlx::Error> {
        // Borrows `&self` directly, with no owned clone, and builds a wrapper
        // context per probe.
        let isolated = op
            .run_isolated(items, async |sp, item| {
                let mut ctx = Ctx {
                    op: sp,
                    label: "batch".to_string(),
                };
                let _ = ctx.label.len();
                self.insert_one(&mut ctx, item).await
            })
            .await?;

        // ...and the bisected flavour, nesting an isolated loop inside a probe.
        let bisected = op
            .run_bisected(items, BisectBudget::Auto, async |sp, slice| {
                sp.run_isolated(slice, async |sp, item| {
                    let _ = self.offset;
                    let _ = sp.maybe_now();
                    Ok::<_, sqlx::Error>(*item)
                })
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .await?;

        Ok(isolated.iter().filter(|o| o.is_ok()).count() + bisected.probes_used)
    }
}

#[tokio::test]
async fn a_closure_borrowing_self_composes_inside_an_async_trait_runner() -> anyhow::Result<()> {
    const BASE: i32 = 7_000;
    let pool = helpers::init_pool().await?;
    reset(&pool, BASE).await?;

    let runner: Arc<dyn BatchRunner> = Arc::new(Runner { offset: BASE });
    let items: Vec<i32> = (0..4).collect();

    // Spawned, so the future is required to be `Send`.
    let handle = tokio::spawn(async move {
        let mut op = DbOp::init(&pool).await?;
        let n = runner.run(&mut op, &items).await?;
        op.commit().await?;
        Ok::<_, sqlx::Error>((n, pool))
    });

    let (n, pool) = handle.await??;
    assert_eq!(n, 5, "4 isolated successes + 1 clean bisect probe");
    assert_eq!(present(&pool, BASE..BASE + 4).await?.len(), 4);

    Ok(())
}
