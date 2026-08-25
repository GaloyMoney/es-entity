mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp, SavepointOperation, WrapsOperation,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};
use std::sync::{Arc, Mutex};

es_entity::entity_id! { SavepointItemId }

fn new_id() -> uuid::Uuid {
    SavepointItemId::new().into()
}

/// Mirrors a real `do_thing_in_op` service method: generic over the operation,
/// so it accepts a `SavepointOp` with no signature change.
async fn insert_item_in_op(
    op: &mut impl AtomicOperation,
    id: uuid::Uuid,
    label: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO savepoint_items (id, label) VALUES ($1, $2)",
        id,
        label
    )
    .execute(op.as_executor())
    .await?;
    Ok(())
}

async fn labels(pool: &sqlx::PgPool, prefix: &str) -> anyhow::Result<Vec<String>> {
    let labels = sqlx::query!(
        "SELECT label FROM savepoint_items WHERE label LIKE $1 ORDER BY label",
        format!("{prefix}%")
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| r.label)
    .collect();
    Ok(labels)
}

/// Records which lifecycle callbacks fired, with the labels carried by the hook.
#[derive(Debug, Clone, Default)]
struct Probe {
    pre: Arc<Mutex<Vec<String>>>,
    post: Arc<Mutex<Vec<String>>>,
    rolled_back: Arc<Mutex<Vec<String>>>,
}

impl Probe {
    fn hook(&self, label: &str) -> MergingProbeHook {
        MergingProbeHook {
            labels: vec![label.to_string()],
            probe: self.clone(),
        }
    }

    fn standalone_hook(&self, label: &str) -> StandaloneProbeHook {
        StandaloneProbeHook {
            label: label.to_string(),
            probe: self.clone(),
        }
    }

    fn pre(&self) -> Vec<String> {
        self.pre.lock().unwrap().clone()
    }

    fn post(&self) -> Vec<String> {
        self.post.lock().unwrap().clone()
    }

    fn rolled_back(&self) -> Vec<String> {
        self.rolled_back.lock().unwrap().clone()
    }
}

/// Stands in for an outbox publisher: always merges, so a whole batch's worth
/// of registrations collapses into one hook.
#[derive(Debug)]
struct MergingProbeHook {
    labels: Vec<String>,
    probe: Probe,
}

impl CommitHook for MergingProbeHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.probe.pre.lock().unwrap().extend(self.labels.clone());
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.probe.post.lock().unwrap().extend(self.labels);
    }

    fn on_rollback(self) {
        self.probe.rolled_back.lock().unwrap().extend(self.labels);
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.labels.append(&mut other.labels);
        true
    }
}

/// Never merges — each registration keeps its own execution slot.
#[derive(Debug)]
struct StandaloneProbeHook {
    label: String,
    probe: Probe,
}

impl CommitHook for StandaloneProbeHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.probe.pre.lock().unwrap().push(self.label.clone());
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.probe.post.lock().unwrap().push(self.label);
    }
}

#[tokio::test]
async fn released_savepoint_folds_staged_hooks_into_parent() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    let res = op
        .with_savepoint(async |op| {
            op.add_commit_hook(probe.hook("item-1")).unwrap();
            Ok::<_, anyhow::Error>(())
        })
        .await?;
    assert!(res.is_ok());

    // Releasing a savepoint must not run any part of the hook lifecycle: the
    // transaction has not committed, so nothing may be announced yet.
    assert!(probe.pre().is_empty());
    assert!(probe.post().is_empty());

    op.commit().await?;

    assert_eq!(probe.pre(), vec!["item-1"]);
    assert_eq!(probe.post(), vec!["item-1"]);
    assert!(probe.rolled_back().is_empty());

    Ok(())
}

#[tokio::test]
async fn rolled_back_savepoint_discards_staged_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    let res = op
        .with_savepoint(async |op| {
            op.add_commit_hook(probe.hook("doomed")).unwrap();
            Err::<(), _>("item failed")
        })
        .await?;
    assert_eq!(res, Err("item failed"));

    op.commit().await?;

    // The item's writes were undone, so its hooks must produce nothing —
    // not even `on_rollback`, which is reserved for a failed *commit*.
    assert!(probe.pre().is_empty());
    assert!(probe.post().is_empty());
    assert!(probe.rolled_back().is_empty());

    Ok(())
}

#[tokio::test]
async fn staged_hooks_merge_across_savepoints() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    for label in ["item-1", "item-2", "item-3"] {
        op.with_savepoint(async |op| {
            op.add_commit_hook(probe.hook(label)).unwrap();
            Ok::<_, anyhow::Error>(())
        })
        .await?
        .unwrap();
    }

    op.commit().await?;

    // One merged hook, accumulated batch-wide in release order — identical to
    // registering all three on the parent directly.
    assert_eq!(probe.pre(), vec!["item-1", "item-2", "item-3"]);
    assert_eq!(probe.post(), vec!["item-1", "item-2", "item-3"]);

    Ok(())
}

#[tokio::test]
async fn only_released_items_contribute_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    for (label, succeeds) in [("item-1", true), ("item-2", false), ("item-3", true)] {
        let res = op
            .with_savepoint(async |op| {
                op.add_commit_hook(probe.hook(label)).unwrap();
                if succeeds { Ok(()) } else { Err("boom") }
            })
            .await?;
        assert_eq!(res.is_ok(), succeeds);
    }

    op.commit().await?;

    assert_eq!(probe.pre(), vec!["item-1", "item-3"]);
    assert_eq!(probe.post(), vec!["item-1", "item-3"]);

    Ok(())
}

#[tokio::test]
async fn absorbed_hooks_keep_parent_registration_order() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    // A merging hook registered before the loop anchors its type's position...
    op.add_commit_hook(probe.hook("pre-loop")).unwrap();
    // ...ahead of a non-merging hook registered after it.
    op.add_commit_hook(probe.standalone_hook("standalone"))
        .unwrap();

    op.with_savepoint(async |op| {
        op.add_commit_hook(probe.hook("from-savepoint")).unwrap();
        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    op.commit().await?;

    // The absorbed hook merges into the pre-loop instance, so it runs at that
    // (earlier) position rather than appending after the standalone hook.
    assert_eq!(
        probe.pre(),
        vec!["pre-loop", "from-savepoint", "standalone"]
    );
    assert_eq!(
        probe.post(),
        vec!["pre-loop", "from-savepoint", "standalone"]
    );

    Ok(())
}

#[tokio::test]
async fn commit_hook_getter_reads_through_to_parent() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    op.add_commit_hook(probe.hook("parent")).unwrap();

    op.with_savepoint(async |op| {
        // Nothing staged yet: the parent's accumulated state is visible.
        let seen = op.commit_hook::<MergingProbeHook>().expect("parent hook");
        assert_eq!(seen.labels, vec!["parent"]);

        op.add_commit_hook(probe.hook("staged")).unwrap();

        // Once staged, the staged instance shadows the parent's until release.
        let seen = op.commit_hook::<MergingProbeHook>().expect("staged hook");
        assert_eq!(seen.labels, vec!["staged"]);

        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    // After release the two are one hook.
    let merged = op.commit_hook::<MergingProbeHook>().expect("merged hook");
    assert_eq!(merged.labels, vec!["parent", "staged"]);

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn later_items_see_earlier_items_accumulated_hook_state() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    op.with_savepoint(async |op| {
        assert!(op.commit_hook::<MergingProbeHook>().is_none());
        op.add_commit_hook(probe.hook("item-1")).unwrap();
        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    op.with_savepoint(async |op| {
        let seen = op
            .commit_hook::<MergingProbeHook>()
            .expect("item-1's hook is visible after its release");
        assert_eq!(seen.labels, vec!["item-1"]);
        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn failed_item_unwinds_its_writes_and_leaves_transaction_usable() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let clashing_id = new_id();

    let mut op = DbOp::init(&pool).await?;

    // Item 1 succeeds.
    op.with_savepoint(async |op| insert_item_in_op(op, clashing_id, &format!("{prefix}-a")).await)
        .await?
        .unwrap();

    // Item 2 writes, then hits a duplicate-key violation — the kind of error
    // that poisons a transaction and would otherwise take the whole batch down.
    let res = op
        .with_savepoint(async |op| {
            insert_item_in_op(op, new_id(), &format!("{prefix}-b")).await?;
            insert_item_in_op(op, clashing_id, &format!("{prefix}-c")).await
        })
        .await?;
    assert!(res.is_err());

    // Item 3 proves the transaction survived the poisoning error.
    op.with_savepoint(async |op| insert_item_in_op(op, new_id(), &format!("{prefix}-d")).await)
        .await?
        .unwrap();

    op.commit().await?;

    // The failed item's *first* write is gone too — savepoint scope, not
    // statement scope.
    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-a"), format!("{prefix}-d")]
    );

    Ok(())
}

#[tokio::test]
async fn batch_loop_collects_per_item_outcomes() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let probe = Probe::default();

    // Two distinct items plus a repeat of the first id — the repeat is the
    // "poison pill" that must fail alone.
    let first_id = new_id();
    let items = vec![(first_id, "a"), (first_id, "b"), (new_id(), "c")];

    let mut op = DbOp::init(&pool).await?;
    let mut outcomes = Vec::with_capacity(items.len());

    for (id, suffix) in items {
        let label = format!("{prefix}-{suffix}");
        // `?` on the outer Result: an infra failure aborts the batch.
        let res = op
            .with_savepoint(async |op| {
                insert_item_in_op(op, id, &label).await?;
                op.add_commit_hook(probe.hook(&label)).unwrap();
                Ok::<_, sqlx::Error>(())
            })
            .await?;

        // The verdict is recorded outside the savepoint, where it is final.
        outcomes.push((suffix, res.is_ok()));
    }

    op.commit().await?;

    assert_eq!(outcomes, vec![("a", true), ("b", false), ("c", true)]);
    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-a"), format!("{prefix}-c")]
    );
    assert_eq!(
        probe.post(),
        vec![format!("{prefix}-a"), format!("{prefix}-c")]
    );

    Ok(())
}

#[tokio::test]
async fn closure_may_mutate_captured_state() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let mut attempted = Vec::new();

    for label in ["item-1", "item-2"] {
        op.with_savepoint(async |op| {
            attempted.push(label);
            sqlx::query!("SELECT 1 as one")
                .fetch_one(op.as_executor())
                .await?;
            Ok::<_, sqlx::Error>(())
        })
        .await?
        .unwrap();
    }

    op.commit().await?;

    assert_eq!(attempted, vec!["item-1", "item-2"]);

    Ok(())
}

#[tokio::test]
async fn savepoint_inherits_cached_time() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let time = "2021-01-01T00:00:00Z".parse::<chrono::DateTime<chrono::Utc>>()?;
    let mut op = DbOp::init(&pool).await?.with_time(time);

    op.with_savepoint(async |op| {
        assert_eq!(op.maybe_now(), Some(time));
        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn nested_savepoint_rolls_back_without_poisoning_parent() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let clashing_id = new_id();

    let mut op = DbOp::init(&pool).await?;

    op.with_savepoint(async |op| {
        insert_item_in_op(op, clashing_id, &format!("{prefix}-outer")).await?;

        // A nested savepoint isolates a sub-item's failure from the item's own
        // (already-isolated) scope.
        let inner_res = op
            .with_savepoint(async |op| {
                insert_item_in_op(op, clashing_id, &format!("{prefix}-inner-doomed")).await
            })
            .await?;
        assert!(inner_res.is_err());

        // The outer item's own writes, and the parent transaction, survive the
        // inner savepoint's rollback.
        insert_item_in_op(op, new_id(), &format!("{prefix}-outer-continues")).await
    })
    .await?
    .unwrap();

    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![
            format!("{prefix}-outer"),
            format!("{prefix}-outer-continues"),
        ]
    );

    Ok(())
}

#[tokio::test]
async fn nested_savepoint_hooks_roll_up_one_parent_at_a_time() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    op.with_savepoint(async |outer| {
        outer.add_commit_hook(probe.hook("outer")).unwrap();

        outer
            .with_savepoint(async |inner| {
                // Not yet visible on the grandparent `DbOp` — only the immediate
                // parent (`outer`) has folded it in, and only once `outer`
                // itself releases.
                inner.add_commit_hook(probe.hook("inner")).unwrap();
                Ok::<_, anyhow::Error>(())
            })
            .await?
            .unwrap();

        // Released into `outer`'s own staged buffer: visible here, on the
        // savepoint it rolled up into, but the root `DbOp` still knows nothing
        // about it until `outer` itself releases.
        let seen = outer
            .commit_hook::<MergingProbeHook>()
            .expect("inner's hook rolled up into outer");
        assert_eq!(seen.labels, vec!["outer", "inner"]);

        Ok::<_, anyhow::Error>(())
    })
    .await?
    .unwrap();

    op.commit().await?;

    // One level up again at the root commit: both merged into a single hook,
    // in release order.
    assert_eq!(probe.pre(), vec!["outer", "inner"]);
    assert_eq!(probe.post(), vec!["outer", "inner"]);

    Ok(())
}

#[tokio::test]
async fn rolled_back_outer_savepoint_discards_already_rolled_up_inner_hooks() -> anyhow::Result<()>
{
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let probe = Probe::default();

    let res = op
        .with_savepoint(async |outer| {
            outer
                .with_savepoint(async |inner| {
                    inner.add_commit_hook(probe.hook("inner")).unwrap();
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .unwrap()
                .unwrap();

            // The inner hook is now staged on `outer`; rolling `outer` back
            // must discard it right along with `outer`'s own writes/hooks.
            Err::<(), _>("outer failed")
        })
        .await?;
    assert_eq!(res, Err("outer failed"));

    op.commit().await?;

    assert!(probe.pre().is_empty());
    assert!(probe.post().is_empty());

    Ok(())
}

#[tokio::test]
async fn explicit_nested_savepoint_release_and_rollback() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let mut op = DbOp::init(&pool).await?;

    let mut outer = op.begin_savepoint().await?;
    insert_item_in_op(&mut outer, new_id(), &format!("{prefix}-outer")).await?;

    let mut inner = outer.begin_savepoint().await?;
    insert_item_in_op(&mut inner, new_id(), &format!("{prefix}-inner-kept")).await?;
    inner.release().await?;

    let mut inner = outer.begin_savepoint().await?;
    insert_item_in_op(&mut inner, new_id(), &format!("{prefix}-inner-undone")).await?;
    inner.rollback().await?;

    outer.release().await?;
    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-inner-kept"), format!("{prefix}-outer")]
    );

    Ok(())
}

#[tokio::test]
async fn explicit_savepoint_release_and_rollback() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let mut op = DbOp::init(&pool).await?;

    let mut sp = op.begin_savepoint().await?;
    insert_item_in_op(&mut sp, new_id(), &format!("{prefix}-kept")).await?;
    sp.release().await?;

    let mut sp = op.begin_savepoint().await?;
    insert_item_in_op(&mut sp, new_id(), &format!("{prefix}-undone")).await?;
    sp.rollback().await?;

    // Dropping without finishing rolls back too.
    {
        let mut sp = op.begin_savepoint().await?;
        insert_item_in_op(&mut sp, new_id(), &format!("{prefix}-dropped")).await?;
    }

    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-kept")]
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Savepoints are derived, not hand-written: these cover operations that had no
// savepoint support at all before `savepoint_parts` + the blanket impl.
// ---------------------------------------------------------------------------

/// An operation defined *outside* `es_entity::operation` — standing in for the
/// wrapper types consumers define (obix's `BatchOp` / `IsolatedOp` / `FlushOp`).
///
/// This is the whole implementation. Two accessors earn the entire
/// `AtomicOperation` surface — time, clock, executor, commit hooks — and with it
/// `SavepointOperation`, nesting included, all carrying the inner operation's
/// real capabilities rather than trait defaults.
struct WrapperOp<'a>(&'a mut DbOp<'static>);

impl<'a> WrapsOperation for WrapperOp<'a> {
    type Inner = DbOp<'static>;

    fn op(&self) -> &Self::Inner {
        self.0
    }

    fn op_mut(&mut self) -> &mut Self::Inner {
        self.0
    }
}

/// Delegation must carry the inner op's real answers, not the trait defaults —
/// the failure mode being a wrapper that silently reports `supports_hooks()
/// == false` and loses the operation's cached time.
#[tokio::test]
async fn wrapping_op_delegates_full_capability() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?.with_db_time().await?;
    let now = op.now();

    // Wrap a DbOpWithTime to prove the associated type is not pinned to DbOp.
    struct TimedWrapper<'a>(&'a mut es_entity::operation::DbOpWithTime<'static>);
    impl<'a> WrapsOperation for TimedWrapper<'a> {
        type Inner = es_entity::operation::DbOpWithTime<'static>;
        fn op(&self) -> &Self::Inner {
            self.0
        }
        fn op_mut(&mut self) -> &mut Self::Inner {
            self.0
        }
    }

    let mut wrapper = TimedWrapper(&mut op);
    assert!(
        wrapper.supports_hooks(),
        "hook support must come from the wrapped op, not the trait default"
    );
    assert_eq!(
        wrapper.maybe_now(),
        Some(now),
        "cached time must survive delegation"
    );

    // And it savepoints, with hooks reaching the root.
    let probe = Probe::default();
    let res = wrapper
        .with_savepoint(async |sp| {
            assert!(sp.supports_hooks());
            assert_eq!(sp.maybe_now(), Some(now));
            sp.add_commit_hook(probe.hook("via-wrapper")).unwrap();
            Ok::<_, sqlx::Error>(())
        })
        .await?;
    assert!(res.is_ok());

    op.commit().await?;
    assert_eq!(probe.post(), vec!["via-wrapper".to_string()]);

    Ok(())
}

/// A foreign operation type earns savepoints — including hook staging that folds
/// all the way through to the root `DbOp` — from `savepoint_parts` alone.
#[tokio::test]
async fn foreign_op_gets_savepoints_and_hook_folding() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let probe = Probe::default();
    let mut op = DbOp::init(&pool).await?;

    {
        let mut wrapper = WrapperOp(&mut op);
        assert!(wrapper.supports_hooks());

        let kept = wrapper
            .with_savepoint(async |sp| {
                assert!(
                    sp.supports_hooks(),
                    "hook support forwards through the wrapper"
                );
                insert_item_in_op(sp, new_id(), &format!("{prefix}-kept")).await?;
                sp.add_commit_hook(probe.hook("kept")).unwrap();
                Ok::<_, sqlx::Error>(())
            })
            .await?;
        assert!(kept.is_ok());

        let undone = wrapper
            .with_savepoint(async |sp| {
                insert_item_in_op(sp, new_id(), &format!("{prefix}-undone")).await?;
                sp.add_commit_hook(probe.hook("undone")).unwrap();
                Err::<(), _>(sqlx::Error::RowNotFound)
            })
            .await?;
        assert!(undone.is_err());
    }

    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-kept")]
    );
    // The rolled-back savepoint's hook never reached the root.
    assert_eq!(probe.post(), vec!["kept".to_string()]);

    Ok(())
}

/// `OpWithTime` had no savepoint methods at all before this change.
#[tokio::test]
async fn op_with_time_gets_savepoints() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let mut op = DbOp::init(&pool).await?;

    {
        let mut timed = es_entity::operation::OpWithTime::cached_or_db_time(&mut op).await?;
        let now = timed.now();

        let res = timed
            .with_savepoint(async |sp| {
                // The cached time propagates into the savepoint.
                assert_eq!(sp.maybe_now(), Some(now));
                insert_item_in_op(sp, new_id(), &format!("{prefix}-kept")).await?;
                Ok::<_, sqlx::Error>(())
            })
            .await?;
        assert!(res.is_ok());
    }

    op.commit().await?;
    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-kept")]
    );

    Ok(())
}

/// A bare `sqlx::Transaction` gets working savepoints, with hooks correctly
/// refused rather than silently swallowed.
#[tokio::test]
async fn bare_transaction_gets_savepoints_without_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let probe = Probe::default();
    let mut tx = pool.begin().await?;

    let res = tx
        .with_savepoint(async |sp| {
            assert!(!sp.supports_hooks(), "no hook buffer to fold into");
            assert!(
                sp.add_commit_hook(probe.hook("refused")).is_err(),
                "registration must refuse so callers take force_execute_pre_commit"
            );
            insert_item_in_op(sp, new_id(), &format!("{prefix}-kept")).await?;
            Ok::<_, sqlx::Error>(())
        })
        .await?;
    assert!(res.is_ok());

    let undone = tx
        .with_savepoint(async |sp| {
            insert_item_in_op(sp, new_id(), &format!("{prefix}-undone")).await?;
            Err::<(), _>(sqlx::Error::RowNotFound)
        })
        .await?;
    assert!(undone.is_err());

    tx.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-kept")]
    );
    assert!(probe.post().is_empty());

    Ok(())
}

/// The reuse story: one generic helper that savepoints per item, called with a
/// `DbOp`, then with a `SavepointOp` (nesting), then with a foreign op — no
/// overloads, no per-op plumbing.
async fn insert_each_isolated(
    op: &mut impl AtomicOperation,
    labels: &[String],
) -> Result<usize, sqlx::Error> {
    let mut kept = 0;
    for label in labels {
        let res = op
            .with_savepoint(async |sp| {
                insert_item_in_op(sp, new_id(), label).await?;
                if label.ends_with("-bad") {
                    return Err(sqlx::Error::RowNotFound);
                }
                Ok(())
            })
            .await?;
        if res.is_ok() {
            kept += 1;
        }
    }
    Ok(kept)
}

#[tokio::test]
async fn generic_helper_works_across_op_types() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let mut op = DbOp::init(&pool).await?;

    // 1. Against a DbOp.
    let kept =
        insert_each_isolated(&mut op, &[format!("{prefix}-a"), format!("{prefix}-b-bad")]).await?;
    assert_eq!(kept, 1);

    // 2. Against a SavepointOp — the same helper, nesting one level deeper.
    let mut sp = op.begin_savepoint().await?;
    let kept =
        insert_each_isolated(&mut sp, &[format!("{prefix}-c"), format!("{prefix}-d-bad")]).await?;
    assert_eq!(kept, 1);
    sp.release().await?;

    // 3. Against a foreign op type.
    {
        let mut wrapper = WrapperOp(&mut op);
        let kept = insert_each_isolated(&mut wrapper, &[format!("{prefix}-e")]).await?;
        assert_eq!(kept, 1);
    }

    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![
            format!("{prefix}-a"),
            format!("{prefix}-c"),
            format!("{prefix}-e")
        ]
    );

    Ok(())
}

/// The generic helper must remain usable inside a `Send` future (a spawned
/// task), which is what most consumers actually do.
#[tokio::test]
async fn generic_savepoint_future_is_send() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());

    let handle = tokio::spawn(async move {
        let mut op = DbOp::init(&pool).await?;
        insert_each_isolated(&mut op, &[format!("{prefix}-spawned")]).await?;
        op.commit().await?;
        Ok::<_, sqlx::Error>(prefix)
    });

    let prefix = handle.await??;
    let pool = helpers::init_pool().await?;
    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-spawned")]
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Hook capability must propagate down a nesting chain.
//
// A savepoint nested inside a savepoint whose own parent cannot receive hooks
// has nowhere to fold them either. If it accepted them anyway, the caller would
// be told the hook was registered while it was silently dropped at release —
// `pre_commit`/`post_commit` never running for work reported as staged.
// ---------------------------------------------------------------------------

/// Nested under a bare `sqlx::Transaction` (no hook buffer anywhere in the
/// chain). Depth 1 already refuses; depth 2 must refuse identically.
#[tokio::test]
async fn nested_savepoint_under_bare_transaction_refuses_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let probe = Probe::default();
    let mut tx = pool.begin().await?;

    let mut outer = tx.begin_savepoint().await?;
    assert!(!outer.supports_hooks());
    assert!(outer.add_commit_hook(probe.hook("depth-1")).is_err());

    let mut inner = outer.begin_savepoint().await?;
    assert!(
        !inner.supports_hooks(),
        "a savepoint nested under a hook-less chain must not claim hook support"
    );
    assert!(
        inner.add_commit_hook(probe.hook("depth-2")).is_err(),
        "registration must fail loudly so the caller takes force_execute_pre_commit \
         instead of believing a hook was staged that will be dropped"
    );

    // Real work still succeeds — only hooks are refused.
    insert_item_in_op(&mut inner, new_id(), &format!("{prefix}-kept")).await?;
    inner.release().await?;
    outer.release().await?;
    tx.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-kept")]
    );
    assert!(probe.pre().is_empty());
    assert!(probe.post().is_empty());

    Ok(())
}

/// Three levels deep under a hook-less root — capability must stay `false` all
/// the way down, not just at depth 2.
#[tokio::test]
async fn deeply_nested_savepoint_under_bare_transaction_refuses_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let probe = Probe::default();
    let mut tx = pool.begin().await?;

    let mut l1 = tx.begin_savepoint().await?;
    let mut l2 = l1.begin_savepoint().await?;
    let mut l3 = l2.begin_savepoint().await?;

    assert!(!l3.supports_hooks());
    assert!(l3.add_commit_hook(probe.hook("depth-3")).is_err());

    l3.release().await?;
    l2.release().await?;
    l1.release().await?;
    tx.commit().await?;

    assert!(probe.post().is_empty());
    Ok(())
}

/// Nested under a `HookOperation` on the `force_execute_pre_commit` path, whose
/// `staged` is `None` — there is no commit pass for a hook to join, at any depth.
#[derive(Debug)]
struct NestingForceExecutedHook {
    probe: Probe,
    accepted_at_depth_2: Arc<Mutex<Option<bool>>>,
    supported_at_depth_2: Arc<Mutex<Option<bool>>>,
}

impl CommitHook for NestingForceExecutedHook {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        assert!(
            !op.supports_hooks(),
            "force_execute_pre_commit path has no commit pass to join"
        );

        let mut outer = op.begin_savepoint().await?;
        assert!(!outer.supports_hooks());

        let mut inner = outer.begin_savepoint().await?;
        *self.supported_at_depth_2.lock().unwrap() = Some(inner.supports_hooks());
        *self.accepted_at_depth_2.lock().unwrap() =
            Some(inner.add_commit_hook(self.probe.hook("depth-2")).is_ok());

        inner.release().await?;
        outer.release().await?;
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn nested_savepoint_under_force_executed_hook_refuses_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let probe = Probe::default();
    let supported = Arc::new(Mutex::new(None));
    let accepted = Arc::new(Mutex::new(None));

    let mut op = DbOp::init(&pool).await?;
    let hook = NestingForceExecutedHook {
        probe: probe.clone(),
        accepted_at_depth_2: accepted.clone(),
        supported_at_depth_2: supported.clone(),
    };
    // Drive the force-execute path directly: `op` here is a plain transaction
    // wrapper with no commit pass, exactly as when `add_commit_hook` refuses.
    let mut tx = pool.begin().await?;
    hook.force_execute_pre_commit(&mut tx).await?;
    tx.commit().await?;
    op.commit().await?;

    assert_eq!(
        *supported.lock().unwrap(),
        Some(false),
        "a savepoint nested under a force-executed HookOperation must not claim hook support"
    );
    assert_eq!(
        *accepted.lock().unwrap(),
        Some(false),
        "registration must fail loudly rather than staging a hook that is then dropped"
    );
    assert!(probe.pre().is_empty());
    assert!(probe.post().is_empty());

    Ok(())
}

// ---------------------------------------------------------------------------
// An enum-dispatching operation — lana's `UseCaseOp` shape. `WrapsOperation`
// cannot serve this (one associated `Inner` cannot cover four variants), so the
// impl is hand-written; `savepoint_parts` is simply one more match in the same
// style as the methods already there. The payoff is that `isolated` below needs
// no per-variant special-casing and no "savepoints unsupported" error: the
// savepoint-backed variant nests, rather than being refused.
// ---------------------------------------------------------------------------

enum UseCaseOp<'op, 'parent> {
    Owned(DbOp<'static>),
    Db(&'op mut DbOp<'static>),
    Savepoint(&'op mut es_entity::operation::SavepointOp<'parent>),
}

impl AtomicOperation for UseCaseOp<'_, '_> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::Owned(op) => op.maybe_now(),
            Self::Db(op) => op.maybe_now(),
            Self::Savepoint(op) => op.maybe_now(),
        }
    }

    fn clock(&self) -> &es_entity::clock::ClockHandle {
        match self {
            Self::Owned(op) => AtomicOperation::clock(op),
            Self::Db(op) => AtomicOperation::clock(*op),
            Self::Savepoint(op) => AtomicOperation::clock(*op),
        }
    }

    fn connection(&mut self) -> &mut es_entity::db::Connection {
        match self {
            Self::Owned(op) => op.connection(),
            Self::Db(op) => op.connection(),
            Self::Savepoint(op) => op.connection(),
        }
    }

    fn add_commit_hook<H: CommitHook>(&mut self, hook: H) -> Result<(), H> {
        match self {
            Self::Owned(op) => op.add_commit_hook(hook),
            Self::Db(op) => op.add_commit_hook(hook),
            Self::Savepoint(op) => op.add_commit_hook(hook),
        }
    }

    fn commit_hook<H: CommitHook>(&self) -> Option<&H> {
        match self {
            Self::Owned(op) => op.commit_hook::<H>(),
            Self::Db(op) => op.commit_hook::<H>(),
            Self::Savepoint(op) => op.commit_hook::<H>(),
        }
    }

    fn supports_hooks(&self) -> bool {
        match self {
            Self::Owned(op) => op.supports_hooks(),
            Self::Db(op) => op.supports_hooks(),
            Self::Savepoint(op) => op.supports_hooks(),
        }
    }

    /// The one addition. Every variant already knows how to yield its parts, so
    /// this is pure dispatch — and it is what lets `isolated` treat all variants
    /// alike, the savepoint-backed one included.
    fn savepoint_parts(
        &mut self,
    ) -> (
        &mut es_entity::db::Connection,
        es_entity::operation::HookSlot<'_>,
    ) {
        match self {
            Self::Owned(op) => op.savepoint_parts(),
            Self::Db(op) => op.savepoint_parts(),
            Self::Savepoint(op) => op.savepoint_parts(),
        }
    }
}

impl UseCaseOp<'_, '_> {
    /// No variant match, no `SavepointsUnsupported`: uniform across all of them.
    async fn isolated<Out, E>(
        &mut self,
        f: impl AsyncFnOnce(&mut UseCaseOp<'_, '_>) -> Result<Out, E>,
    ) -> Result<Result<Out, E>, sqlx::Error> {
        self.with_savepoint(async |sp| {
            let mut child = UseCaseOp::Savepoint(sp);
            f(&mut child).await
        })
        .await
    }
}

#[tokio::test]
async fn enum_op_isolates_uniformly_across_variants() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let probe = Probe::default();
    let mut root = DbOp::init(&pool).await?;

    {
        // Borrowed-DbOp variant: one level.
        let mut ctx = UseCaseOp::Db(&mut root);
        assert!(ctx.supports_hooks());

        let kept = ctx
            .isolated(async |child| {
                insert_item_in_op(child, new_id(), &format!("{prefix}-a")).await?;
                child.add_commit_hook(probe.hook("a")).unwrap();
                Ok::<_, sqlx::Error>(())
            })
            .await?;
        assert!(kept.is_ok());

        // Savepoint-backed variant nesting inside another isolation — the case
        // that previously had to return SavepointsUnsupported.
        let nested = ctx
            .isolated(async |child| {
                insert_item_in_op(child, new_id(), &format!("{prefix}-b")).await?;

                let inner_ok = child
                    .isolated(async |grandchild| {
                        insert_item_in_op(grandchild, new_id(), &format!("{prefix}-c")).await?;
                        grandchild.add_commit_hook(probe.hook("c")).unwrap();
                        Ok::<_, sqlx::Error>(())
                    })
                    .await?;
                assert!(inner_ok.is_ok());

                // A failing grandchild unwinds only itself.
                let inner_bad = child
                    .isolated(async |grandchild| {
                        insert_item_in_op(grandchild, new_id(), &format!("{prefix}-d")).await?;
                        grandchild.add_commit_hook(probe.hook("d")).unwrap();
                        Err::<(), _>(sqlx::Error::RowNotFound)
                    })
                    .await?;
                assert!(inner_bad.is_err());

                child.add_commit_hook(probe.hook("b")).unwrap();
                Ok::<_, sqlx::Error>(())
            })
            .await?;
        assert!(nested.is_ok());
    }

    root.commit().await?;

    // `-d` rolled back with its grandchild savepoint; everything else survived.
    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![
            format!("{prefix}-a"),
            format!("{prefix}-b"),
            format!("{prefix}-c")
        ]
    );
    // Hooks rolled up through both levels, in release order, minus the failed one.
    assert_eq!(probe.post(), vec!["a", "c", "b"]);

    Ok(())
}

#[tokio::test]
async fn enum_op_isolates_from_owned_variant() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let prefix = format!("sp-{}", new_id());
    let mut ctx = UseCaseOp::Owned(DbOp::init(&pool).await?);

    let res = ctx
        .isolated(async |child| {
            insert_item_in_op(child, new_id(), &format!("{prefix}-owned")).await?;
            Ok::<_, sqlx::Error>(())
        })
        .await?;
    assert!(res.is_ok());

    let UseCaseOp::Owned(op) = ctx else {
        unreachable!()
    };
    op.commit().await?;

    assert_eq!(
        labels(&pool, &prefix).await?,
        vec![format!("{prefix}-owned")]
    );
    Ok(())
}
