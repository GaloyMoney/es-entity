mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp,
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
