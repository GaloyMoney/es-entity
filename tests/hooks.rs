mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp, OpWithTime,
    hooks::{CommitHook, HookOperation, MAX_HOOK_GENERATIONS, PreCommitRet},
};
use sqlx::Connection;
use std::{
    any::TypeId,
    sync::{Arc, Mutex},
};

#[derive(Debug)]
struct FullCommitHook {
    data: String,
    pre_result: Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>,
    post_result: Arc<Mutex<String>>,
}

impl CommitHook for FullCommitHook {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        let result = sqlx::query!("SELECT NOW() as now")
            .fetch_one(op.as_executor())
            .await?;
        *self.pre_result.lock().unwrap() = result.now;
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        *self.post_result.lock().unwrap() = format!("post:{}", self.data);
    }
}

#[tokio::test]
async fn both_pre_and_post_commit_execute() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_result = Arc::new(Mutex::new(None));
    let post_result = Arc::new(Mutex::new(String::new()));

    op.add_commit_hook(FullCommitHook {
        data: "test".to_string(),
        pre_result: pre_result.clone(),
        post_result: post_result.clone(),
    })
    .unwrap();

    assert!(pre_result.lock().unwrap().is_none());
    op.commit().await?;

    let captured_time = pre_result
        .lock()
        .unwrap()
        .expect("should have captured db time");
    let now = chrono::Utc::now();
    assert!(now.signed_duration_since(captured_time).num_seconds().abs() < 5);
    assert_eq!(*post_result.lock().unwrap(), "post:test");

    Ok(())
}

#[derive(Debug)]
struct MergeableEvents {
    events: Vec<String>,
    pre_result: Arc<Mutex<Vec<String>>>,
    post_result: Arc<Mutex<Vec<String>>>,
}

impl CommitHook for MergeableEvents {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        *self.pre_result.lock().unwrap() = self.events.clone();
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        *self.post_result.lock().unwrap() = self.events;
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.events.append(&mut other.events);
        true
    }
}

#[tokio::test]
async fn hooks_merge_when_returning_true() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_result = Arc::new(Mutex::new(Vec::new()));
    let post_result = Arc::new(Mutex::new(Vec::new()));

    op.add_commit_hook(MergeableEvents {
        events: vec!["e1".into()],
        pre_result: pre_result.clone(),
        post_result: post_result.clone(),
    })
    .unwrap();
    op.add_commit_hook(MergeableEvents {
        events: vec!["e2".into(), "e3".into()],
        pre_result: pre_result.clone(),
        post_result: post_result.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(*pre_result.lock().unwrap(), vec!["e1", "e2", "e3"]);
    assert_eq!(*post_result.lock().unwrap(), vec!["e1", "e2", "e3"]);

    Ok(())
}

#[derive(Debug)]
struct NonMergeableHook {
    pre_count: Arc<Mutex<i32>>,
    post_count: Arc<Mutex<i32>>,
}

impl CommitHook for NonMergeableHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        *self.pre_count.lock().unwrap() += 1;
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        *self.post_count.lock().unwrap() += 1;
    }
}

#[tokio::test]
async fn hooks_execute_separately_when_not_merged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_count = Arc::new(Mutex::new(0));
    let post_count = Arc::new(Mutex::new(0));

    op.add_commit_hook(NonMergeableHook {
        pre_count: pre_count.clone(),
        post_count: post_count.clone(),
    })
    .unwrap();
    op.add_commit_hook(NonMergeableHook {
        pre_count: pre_count.clone(),
        post_count: post_count.clone(),
    })
    .unwrap();
    op.add_commit_hook(NonMergeableHook {
        pre_count: pre_count.clone(),
        post_count: post_count.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(*pre_count.lock().unwrap(), 3);
    assert_eq!(*post_count.lock().unwrap(), 3);

    Ok(())
}

#[derive(Debug)]
struct MergingGetterHook {
    payloads: Vec<String>,
}

impl CommitHook for MergingGetterHook {
    fn merge(&mut self, other: &mut Self) -> bool {
        self.payloads.append(&mut other.payloads);
        true
    }
}

#[derive(Debug)]
struct NonMergingGetterHook {
    label: &'static str,
}

impl CommitHook for NonMergingGetterHook {}

#[tokio::test]
async fn commit_hook_returns_registered_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    assert!(op.commit_hook::<MergingGetterHook>().is_none());

    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e1".into()],
    })
    .unwrap();

    let hook = op
        .commit_hook::<MergingGetterHook>()
        .expect("hook should be registered");
    assert_eq!(hook.payloads, vec!["e1"]);

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn commit_hook_returns_none_for_different_type() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e1".into()],
    })
    .unwrap();

    assert!(op.commit_hook::<NonMergingGetterHook>().is_none());

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn commit_hook_sees_merged_contents() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e1".into()],
    })
    .unwrap();
    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e2".into(), "e3".into()],
    })
    .unwrap();

    let hook = op
        .commit_hook::<MergingGetterHook>()
        .expect("hook should be registered");
    assert_eq!(hook.payloads, vec!["e1", "e2", "e3"]);

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn commit_hook_returns_last_non_merging_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    op.add_commit_hook(NonMergingGetterHook { label: "first" })
        .unwrap();
    op.add_commit_hook(NonMergingGetterHook { label: "second" })
        .unwrap();

    let hook = op
        .commit_hook::<NonMergingGetterHook>()
        .expect("hook should be registered");
    assert_eq!(hook.label, "second");

    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn commit_hook_default_returns_none_for_bare_transaction() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tx = pool.begin().await?;

    assert!(tx.commit_hook::<MergingGetterHook>().is_none());

    tx.commit().await?;

    Ok(())
}

#[tokio::test]
async fn commit_hook_delegates_through_time_wrappers() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let op = DbOp::init(&pool).await?;
    let mut op = op.with_db_time().await?;

    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e1".into()],
    })
    .unwrap();

    let hook = op
        .commit_hook::<MergingGetterHook>()
        .expect("DbOpWithTime should delegate to inner op");
    assert_eq!(hook.payloads, vec!["e1"]);

    let wrapped = OpWithTime::cached_or_clock_time(&mut op);
    let hook = wrapped
        .commit_hook::<MergingGetterHook>()
        .expect("OpWithTime should delegate to wrapped op");
    assert_eq!(hook.payloads, vec!["e1"]);
    drop(wrapped);

    op.commit().await?;

    Ok(())
}

#[derive(Debug)]
struct SiblingProbeHook {
    saw_sibling: Arc<Mutex<Option<bool>>>,
}

impl CommitHook for SiblingProbeHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        *self.saw_sibling.lock().unwrap() = Some(op.commit_hook::<MergingGetterHook>().is_some());
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn commit_hook_not_visible_inside_pre_commit() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let saw_sibling = Arc::new(Mutex::new(None));

    op.add_commit_hook(MergingGetterHook {
        payloads: vec!["e1".into()],
    })
    .unwrap();
    op.add_commit_hook(SiblingProbeHook {
        saw_sibling: saw_sibling.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(*saw_sibling.lock().unwrap(), Some(false));

    Ok(())
}

#[derive(Debug)]
struct OrderProbe<const N: usize> {
    label: &'static str,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
}

impl<const N: usize> CommitHook for OrderProbe<N> {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push(self.label);
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().push(self.label);
    }
}

#[derive(Debug)]
struct MergingOrderProbe {
    labels: Vec<&'static str>,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for MergingOrderProbe {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().extend(&self.labels);
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().extend(&self.labels);
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.labels.append(&mut other.labels);
        true
    }
}

#[tokio::test]
async fn hooks_execute_in_registration_order_across_types() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // Repeat to catch nondeterministic ordering: with the previous
    // HashMap-backed storage, cross-type execution order differed between
    // iterations; the registration-order contract must hold on every run.
    for _ in 0..100 {
        let pre_order = Arc::new(Mutex::new(Vec::new()));
        let post_order = Arc::new(Mutex::new(Vec::new()));
        let mut op = DbOp::init(&pool).await?;

        macro_rules! probe {
            ($n:literal, $label:literal) => {
                op.add_commit_hook(OrderProbe::<$n> {
                    label: $label,
                    pre_order: pre_order.clone(),
                    post_order: post_order.clone(),
                })
                .unwrap();
            };
        }
        probe!(1, "one");
        probe!(2, "two");
        probe!(3, "three");
        probe!(4, "four");
        probe!(5, "five");

        op.commit().await?;

        let expected = vec!["one", "two", "three", "four", "five"];
        assert_eq!(*pre_order.lock().unwrap(), expected);
        assert_eq!(*post_order.lock().unwrap(), expected);
    }

    Ok(())
}

#[tokio::test]
async fn merged_hook_executes_at_position_of_first_registration() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    // A, B, A′ where A′ merges into A: the merged hook keeps A's position.
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["a1"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(OrderProbe::<10> {
        label: "b",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["a2"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    let expected = vec!["a1", "a2", "b"];
    assert_eq!(*pre_order.lock().unwrap(), expected);
    assert_eq!(*post_order.lock().unwrap(), expected);

    Ok(())
}

#[tokio::test]
async fn non_merging_hook_executes_at_own_registration_position() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    // A, B, A″ where A″ refuses to merge (default merge() == false): A″ runs at
    // its own later position instead of being grouped with A.
    op.add_commit_hook(OrderProbe::<20> {
        label: "a1",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(OrderProbe::<21> {
        label: "b",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(OrderProbe::<20> {
        label: "a2",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    let expected = vec!["a1", "b", "a2"];
    assert_eq!(*pre_order.lock().unwrap(), expected);
    assert_eq!(*post_order.lock().unwrap(), expected);

    Ok(())
}

#[tokio::test]
async fn post_commit_order_mirrors_pre_commit_order() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    // Mixed merging and non-merging registrations.
    op.add_commit_hook(OrderProbe::<30> {
        label: "x",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["m1"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(OrderProbe::<31> {
        label: "y",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["m2"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    let pre = pre_order.lock().unwrap().clone();
    let post = post_order.lock().unwrap().clone();
    assert_eq!(pre, vec!["x", "m1", "m2", "y"]);
    assert_eq!(post, pre);

    Ok(())
}

#[tokio::test]
async fn supports_hooks_reflects_op_capability() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // A DbOp is backed by a commit-hook buffer.
    let op = DbOp::init(&pool).await?;
    assert!(op.supports_hooks());

    // Time wrappers delegate to the inner op. (The `OpWithTime` temporary
    // borrows `with_time` only for the duration of the statement.)
    let mut with_time = op.with_db_time().await?;
    assert!(with_time.supports_hooks());
    assert!(OpWithTime::cached_or_clock_time(&mut with_time).supports_hooks());
    with_time.commit().await?;

    // A bare sqlx::Transaction has no hook buffer, so it reports no support —
    // distinct from a hook-capable op that merely has nothing registered yet
    // (both of which `commit_hook` would report as `None`).
    let tx = pool.begin().await?;
    assert!(!tx.supports_hooks());
    tx.rollback().await?;

    Ok(())
}

// ===========================================================================
// on_rollback tests
// ===========================================================================

/// Records lifecycle callbacks by label. Optionally fails its own `pre_commit`
/// (to drive the rollback path for the *other*, earlier hooks).
#[derive(Debug)]
struct RollbackProbe {
    label: &'static str,
    fail_pre: bool,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
    rollback_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for RollbackProbe {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push(self.label);
        if self.fail_pre {
            return Err(sqlx::Error::Protocol(format!(
                "hook '{}' intentionally fails pre_commit",
                self.label
            )));
        }
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().push(self.label);
    }

    fn on_rollback(self) {
        self.rollback_order.lock().unwrap().push(self.label);
    }
}

#[tokio::test]
async fn on_rollback_fires_for_earlier_hooks_when_later_hook_fails() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let post = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    let mk = |label: &'static str, fail_pre: bool| RollbackProbe {
        label,
        fail_pre,
        pre_order: pre.clone(),
        post_order: post.clone(),
        rollback_order: rollback.clone(),
    };

    op.add_commit_hook(mk("a", false)).unwrap();
    op.add_commit_hook(mk("b", true)).unwrap();
    // Registered after the failing hook: its pre_commit never runs.
    op.add_commit_hook(mk("c", false)).unwrap();

    let result = op.commit().await;
    assert!(
        result.is_err(),
        "commit must fail because hook 'b' fails pre_commit"
    );

    // 'a' and 'b' ran pre_commit; 'c' never did (the loop stops at the failure).
    assert_eq!(*pre.lock().unwrap(), vec!["a", "b"]);
    // Only 'a' is notified: 'b' is consumed by its own failing pre_commit and
    // signals from that branch; 'c' never ran.
    assert_eq!(*rollback.lock().unwrap(), vec!["a"]);
    // No post_commit on the failure path.
    assert!(post.lock().unwrap().is_empty());

    Ok(())
}

#[tokio::test]
async fn on_rollback_not_fired_on_commit_success() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let post = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    op.add_commit_hook(RollbackProbe {
        label: "a",
        fail_pre: false,
        pre_order: pre.clone(),
        post_order: post.clone(),
        rollback_order: rollback.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(*pre.lock().unwrap(), vec!["a"]);
    assert_eq!(*post.lock().unwrap(), vec!["a"]);
    assert!(
        rollback.lock().unwrap().is_empty(),
        "on_rollback must not fire on a successful commit"
    );

    Ok(())
}

#[tokio::test]
async fn on_rollback_not_fired_when_op_dropped_without_commit() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let post = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    {
        let mut op = DbOp::init(&pool).await?;
        op.add_commit_hook(RollbackProbe {
            label: "a",
            fail_pre: false,
            pre_order: pre.clone(),
            post_order: post.clone(),
            rollback_order: rollback.clone(),
        })
        .unwrap();
        // `op` is dropped here without commit(): pre_commit never ran, so there
        // is nothing to compensate.
    }

    assert!(
        pre.lock().unwrap().is_empty(),
        "pre_commit only runs at commit()"
    );
    assert!(post.lock().unwrap().is_empty());
    assert!(
        rollback.lock().unwrap().is_empty(),
        "on_rollback must not fire for an op dropped without commit()"
    );

    Ok(())
}

#[tokio::test]
async fn on_rollback_fires_in_registration_order() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let post = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    let mk = |label: &'static str, fail_pre: bool| RollbackProbe {
        label,
        fail_pre,
        pre_order: pre.clone(),
        post_order: post.clone(),
        rollback_order: rollback.clone(),
    };

    op.add_commit_hook(mk("a1", false)).unwrap();
    op.add_commit_hook(mk("a2", false)).unwrap();
    op.add_commit_hook(mk("a3", false)).unwrap();
    op.add_commit_hook(mk("boom", true)).unwrap();

    assert!(op.commit().await.is_err());

    // on_rollback mirrors post_commit's registration order.
    assert_eq!(*rollback.lock().unwrap(), vec!["a1", "a2", "a3"]);
    assert!(post.lock().unwrap().is_empty());

    Ok(())
}

/// A hook that does NOT override `on_rollback`, exercising the defaulted no-op.
#[derive(Debug)]
struct DefaultOnRollbackHook {
    pre_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for DefaultOnRollbackHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push("default");
        PreCommitRet::ok(self, op)
    }
    // `on_rollback` intentionally not overridden — uses the default no-op.
}

#[tokio::test]
async fn default_on_rollback_is_a_harmless_noop() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    // Earlier hook relies on the DEFAULT on_rollback (no override).
    op.add_commit_hook(DefaultOnRollbackHook {
        pre_order: pre.clone(),
    })
    .unwrap();
    // Later hook fails, so the earlier hook's default no-op on_rollback fires.
    op.add_commit_hook(RollbackProbe {
        label: "boom",
        fail_pre: true,
        pre_order: pre.clone(),
        post_order: Arc::new(Mutex::new(Vec::new())),
        rollback_order: rollback.clone(),
    })
    .unwrap();

    assert!(op.commit().await.is_err());

    // The default hook ran pre_commit and its default no-op on_rollback did not
    // panic; the failing hook is consumed by its own failure.
    assert_eq!(*pre.lock().unwrap(), vec!["default", "boom"]);
    assert!(rollback.lock().unwrap().is_empty());

    Ok(())
}

/// Holds a transaction-scoped advisory lock in `pre_commit`; when its
/// `on_rollback` fires it probes, from a *separate* connection, whether the lock
/// is free — which it can only be if the operation's transaction has already
/// been rolled back. Proves `on_rollback` runs strictly after the rollback.
#[derive(Debug)]
struct AdvisoryLockProbe {
    lock_key: i64,
    /// `Some(true)` iff the second connection acquired the lock inside on_rollback.
    acquired_after_rollback: Arc<Mutex<Option<bool>>>,
}

impl CommitHook for AdvisoryLockProbe {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        // Transaction-scoped: auto-released only when this operation's tx ends.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(self.lock_key)
            .execute(op.as_executor())
            .await?;
        PreCommitRet::ok(self, op)
    }

    fn on_rollback(self) {
        // The callback is sync, so run the async probe on its own thread with a
        // dedicated runtime and a fresh connection — fully independent of the
        // test's runtime and connection pool. A plain SELECT of the row is not a
        // valid probe (under READ COMMITTED an uncommitted row is invisible to
        // other sessions whether the tx is open or rolled back); lock contention
        // is what distinguishes the two.
        let key = self.lock_key;
        let acquired = std::thread::spawn(move || {
            let pg_con = std::env::var("PG_CON").expect("PG_CON must be set");
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build probe runtime");
            rt.block_on(async move {
                let mut conn = sqlx::PgConnection::connect(&pg_con)
                    .await
                    .expect("probe connection");
                let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                    .bind(key)
                    .fetch_one(&mut conn)
                    .await
                    .expect("advisory lock probe");
                // `conn` drops at scope end, releasing any session lock acquired.
                got
            })
        })
        .join()
        .expect("probe thread panicked");

        *self.acquired_after_rollback.lock().unwrap() = Some(acquired);
    }
}

#[tokio::test]
async fn on_rollback_runs_after_transaction_is_rolled_back() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    // A distinctive key unlikely to collide with concurrent tests.
    let lock_key: i64 = 728_192_374_651;
    let acquired = Arc::new(Mutex::new(None));

    op.add_commit_hook(AdvisoryLockProbe {
        lock_key,
        acquired_after_rollback: acquired.clone(),
    })
    .unwrap();

    // A later hook fails, forcing the rollback-then-notify path for the probe.
    let sink = Arc::new(Mutex::new(Vec::new()));
    op.add_commit_hook(RollbackProbe {
        label: "boom",
        fail_pre: true,
        pre_order: sink.clone(),
        post_order: sink.clone(),
        rollback_order: sink.clone(),
    })
    .unwrap();

    let result = op.commit().await;
    assert!(
        result.is_err(),
        "commit must fail due to the later failing hook"
    );

    let acquired = acquired
        .lock()
        .unwrap()
        .expect("on_rollback should have run and probed the advisory lock");
    assert!(
        acquired,
        "the second connection must acquire the advisory lock inside on_rollback — \
         proving the operation's transaction had already rolled back (releasing the lock) \
         before on_rollback fired"
    );

    Ok(())
}

// ===========================================================================
// Re-entrant registration tests
// ===========================================================================

/// A hook whose `pre_commit` registers a fresh [`NonMergingGetterHook`]-shaped
/// child (non-mergeable) via [`AtomicOperation::add_commit_hook`] on the
/// [`HookOperation`] it is handed.
#[derive(Debug)]
struct ChildHook {
    label: &'static str,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for ChildHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push(self.label);
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().push(self.label);
    }
}

#[derive(Debug)]
struct RegistersChild {
    label: &'static str,
    child_label: &'static str,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for RegistersChild {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push(self.label);
        op.add_commit_hook(ChildHook {
            label: self.child_label,
            pre_order: self.pre_order.clone(),
            post_order: self.post_order.clone(),
        })
        .expect("a real commit pass must accept re-entrant registration");
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().push(self.label);
    }
}

#[tokio::test]
async fn reentrant_hook_runs_pre_and_post_commit_in_the_same_pass() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    op.add_commit_hook(RegistersChild {
        label: "parent",
        child_label: "child",
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    // The re-entrantly registered child joins the tail of the same pass: it
    // runs its own pre_commit before the single COMMIT, and its post_commit
    // after — exactly like a hook registered on the operation directly.
    assert_eq!(*pre_order.lock().unwrap(), vec!["parent", "child"]);
    assert_eq!(*post_order.lock().unwrap(), vec!["parent", "child"]);

    Ok(())
}

#[tokio::test]
async fn reentrant_hook_merges_into_a_still_pending_same_type_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    #[derive(Debug)]
    struct RegistersMergeable {
        label: &'static str,
        staged_labels: Vec<&'static str>,
        pre_order: Arc<Mutex<Vec<&'static str>>>,
        post_order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl CommitHook for RegistersMergeable {
        async fn pre_commit(
            self,
            mut op: HookOperation<'_>,
        ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
            self.pre_order.lock().unwrap().push(self.label);
            op.add_commit_hook(MergingOrderProbe {
                labels: self.staged_labels.clone(),
                pre_order: self.pre_order.clone(),
                post_order: self.post_order.clone(),
            })
            .expect("a real commit pass must accept re-entrant registration");
            PreCommitRet::ok(self, op)
        }

        fn post_commit(self) {
            self.post_order.lock().unwrap().push(self.label);
        }
    }

    // "first" runs before the native MergingOrderProbe and re-entrantly stages
    // one of the same type. The native one is still pending at that point, so
    // the staged instance merges into it — one execution, at the native
    // hook's own (later) queue position — instead of a second, separate one.
    op.add_commit_hook(RegistersMergeable {
        label: "first",
        staged_labels: vec!["staged"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["native"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(
        *pre_order.lock().unwrap(),
        vec!["first", "native", "staged"]
    );
    assert_eq!(
        *post_order.lock().unwrap(),
        vec!["first", "native", "staged"]
    );

    Ok(())
}

#[tokio::test]
async fn reentrant_hook_of_an_already_executed_type_appends_a_fresh_instance() -> anyhow::Result<()>
{
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_order = Arc::new(Mutex::new(Vec::new()));
    let post_order = Arc::new(Mutex::new(Vec::new()));

    #[derive(Debug)]
    struct RegistersMergeable {
        label: &'static str,
        staged_labels: Vec<&'static str>,
        pre_order: Arc<Mutex<Vec<&'static str>>>,
        post_order: Arc<Mutex<Vec<&'static str>>>,
    }

    impl CommitHook for RegistersMergeable {
        async fn pre_commit(
            self,
            mut op: HookOperation<'_>,
        ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
            self.pre_order.lock().unwrap().push(self.label);
            op.add_commit_hook(MergingOrderProbe {
                labels: self.staged_labels.clone(),
                pre_order: self.pre_order.clone(),
                post_order: self.post_order.clone(),
            })
            .expect("a real commit pass must accept re-entrant registration");
            PreCommitRet::ok(self, op)
        }

        fn post_commit(self) {
            self.post_order.lock().unwrap().push(self.label);
        }
    }

    // The native MergingOrderProbe now runs FIRST and has already executed
    // by the time "second" re-entrantly stages one of the same type — an
    // already-executed hook is never a merge target, so the staged instance
    // starts a fresh execution of its own at the tail instead of vanishing
    // into (or reopening) the one that already ran.
    op.add_commit_hook(MergingOrderProbe {
        labels: vec!["native"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();
    op.add_commit_hook(RegistersMergeable {
        label: "second",
        staged_labels: vec!["staged"],
        pre_order: pre_order.clone(),
        post_order: post_order.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(
        *pre_order.lock().unwrap(),
        vec!["native", "second", "staged"]
    );
    assert_eq!(
        *post_order.lock().unwrap(),
        vec!["native", "second", "staged"]
    );

    Ok(())
}

#[derive(Debug)]
struct ForcePathProbe {
    supports_hooks_result: Arc<Mutex<Option<bool>>>,
    add_hook_result: Arc<Mutex<Option<bool>>>,
}

impl CommitHook for ForcePathProbe {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        *self.supports_hooks_result.lock().unwrap() = Some(op.supports_hooks());
        let registered = op
            .add_commit_hook(NonMergingGetterHook { label: "child" })
            .is_ok();
        *self.add_hook_result.lock().unwrap() = Some(registered);
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn force_execute_path_still_refuses_reentrant_registration() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?; // bare sqlx::Transaction: no hook support at all

    let supports_hooks_result = Arc::new(Mutex::new(None));
    let add_hook_result = Arc::new(Mutex::new(None));

    ForcePathProbe {
        supports_hooks_result: supports_hooks_result.clone(),
        add_hook_result: add_hook_result.clone(),
    }
    .force_execute_pre_commit(&mut tx)
    .await?;

    // The force-execute escape hatch has no commit pass for a registered
    // hook to join, so it must keep refusing — a caller that gets `Err` from
    // `add_commit_hook` needs to take its own immediate-execution fallback,
    // not have the hook silently accepted and its post_commit/on_rollback
    // never run.
    assert_eq!(*supports_hooks_result.lock().unwrap(), Some(false));
    assert_eq!(*add_hook_result.lock().unwrap(), Some(false));

    tx.rollback().await?;

    Ok(())
}

/// A hook whose `pre_commit` isolates its own multi-row write in a nested
/// `SAVEPOINT`, discarding one row without losing the others or poisoning the
/// enclosing commit pass.
#[derive(Debug)]
struct SavepointingHook {
    prefix: String,
    clashing_id: uuid::Uuid,
}

impl CommitHook for SavepointingHook {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        sqlx::query!(
            "INSERT INTO savepoint_items (id, label) VALUES ($1, $2)",
            self.clashing_id,
            format!("{}-kept", self.prefix)
        )
        .execute(op.as_executor())
        .await?;

        let doomed_res = op
            .with_savepoint(async |sp| {
                sqlx::query!(
                    "INSERT INTO savepoint_items (id, label) VALUES ($1, $2)",
                    self.clashing_id,
                    format!("{}-doomed", self.prefix)
                )
                .execute(sp.as_executor())
                .await
            })
            .await?;
        assert!(doomed_res.is_err(), "duplicate id must fail the insert");

        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn hook_pre_commit_savepoint_isolates_its_own_write() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;
    let prefix = format!("sp-hook-{}", uuid::Uuid::now_v7());

    op.add_commit_hook(SavepointingHook {
        prefix: prefix.clone(),
        clashing_id: uuid::Uuid::now_v7(),
    })
    .unwrap();

    // Proves the hook's rolled-back savepoint didn't poison the commit pass's
    // own transaction.
    op.commit().await?;

    let labels: Vec<String> = sqlx::query!(
        "SELECT label FROM savepoint_items WHERE label LIKE $1 ORDER BY label",
        format!("{prefix}%")
    )
    .fetch_all(&pool)
    .await?
    .into_iter()
    .map(|r| r.label)
    .collect();
    assert_eq!(labels, vec![format!("{prefix}-kept")]);

    Ok(())
}

/// On the `force_execute_pre_commit` path there is no commit pass for a
/// registered hook to join, so `add_commit_hook` inside a nested savepoint
/// must keep refusing exactly like it does directly on the `HookOperation` —
/// but the raw `SAVEPOINT`/`RELEASE`/`ROLLBACK` machinery, needing only the
/// connection, works regardless.
#[derive(Debug)]
struct ForcePathSavepointProbe {
    prefix: String,
    add_hook_result: Arc<Mutex<Option<bool>>>,
}

impl CommitHook for ForcePathSavepointProbe {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        let mut sp = op.begin_savepoint().await?;

        let registered = sp
            .add_commit_hook(NonMergingGetterHook { label: "child" })
            .is_ok();
        *self.add_hook_result.lock().unwrap() = Some(registered);

        sqlx::query!(
            "INSERT INTO savepoint_items (id, label) VALUES ($1, $2)",
            uuid::Uuid::now_v7(),
            format!("{}-forced", self.prefix)
        )
        .execute(sp.as_executor())
        .await?;
        sp.release().await?;

        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn force_execute_path_savepoint_works_without_hook_support() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?; // bare sqlx::Transaction: no hook support at all
    let prefix = format!("sp-force-{}", uuid::Uuid::now_v7());

    let add_hook_result = Arc::new(Mutex::new(None));

    ForcePathSavepointProbe {
        prefix: prefix.clone(),
        add_hook_result: add_hook_result.clone(),
    }
    .force_execute_pre_commit(&mut tx)
    .await?;

    assert_eq!(*add_hook_result.lock().unwrap(), Some(false));

    tx.commit().await?;

    let labels: Vec<String> = sqlx::query!(
        "SELECT label FROM savepoint_items WHERE label LIKE $1 ORDER BY label",
        format!("{prefix}%")
    )
    .fetch_all(&pool)
    .await?
    .into_iter()
    .map(|r| r.label)
    .collect();
    assert_eq!(labels, vec![format!("{prefix}-forced")]);

    Ok(())
}

#[derive(Debug)]
struct StagesFailingChild {
    label: &'static str,
    child_label: &'static str,
    pre_order: Arc<Mutex<Vec<&'static str>>>,
    post_order: Arc<Mutex<Vec<&'static str>>>,
    rollback_order: Arc<Mutex<Vec<&'static str>>>,
}

impl CommitHook for StagesFailingChild {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_order.lock().unwrap().push(self.label);
        op.add_commit_hook(RollbackProbe {
            label: self.child_label,
            fail_pre: true,
            pre_order: self.pre_order.clone(),
            post_order: self.post_order.clone(),
            rollback_order: self.rollback_order.clone(),
        })
        .expect("a real commit pass must accept re-entrant registration");
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post_order.lock().unwrap().push(self.label);
    }

    fn on_rollback(self) {
        self.rollback_order.lock().unwrap().push(self.label);
    }
}

#[tokio::test]
async fn reentrant_failure_rolls_back_and_notifies_the_staging_parent() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre = Arc::new(Mutex::new(Vec::new()));
    let post = Arc::new(Mutex::new(Vec::new()));
    let rollback = Arc::new(Mutex::new(Vec::new()));

    op.add_commit_hook(RollbackProbe {
        label: "a",
        fail_pre: false,
        pre_order: pre.clone(),
        post_order: post.clone(),
        rollback_order: rollback.clone(),
    })
    .unwrap();
    // "parent" re-entrantly stages "child", whose pre_commit fails.
    op.add_commit_hook(StagesFailingChild {
        label: "parent",
        child_label: "child",
        pre_order: pre.clone(),
        post_order: post.clone(),
        rollback_order: rollback.clone(),
    })
    .unwrap();

    let result = op.commit().await;
    assert!(
        result.is_err(),
        "commit must fail because the re-entrantly staged child fails pre_commit"
    );

    // 'a' and 'parent' ran pre_commit natively; 'child' ran too (re-entrantly
    // staged, at the tail) and is the one whose pre_commit failed.
    assert_eq!(*pre.lock().unwrap(), vec!["a", "parent", "child"]);
    // Everyone whose pre_commit already completed is notified of the
    // rollback — including 'parent', the hook that staged the failing
    // child, exactly as it would be for a hook registered on the operation
    // directly. 'child' is consumed by its own failure and signals (or, here,
    // doesn't) from that branch instead.
    assert_eq!(*rollback.lock().unwrap(), vec!["a", "parent"]);
    assert!(post.lock().unwrap().is_empty());

    Ok(())
}

/// Stages a fresh instance of itself, one generation deeper, every time it
/// runs — an unbroken re-registration cycle.
#[derive(Debug)]
struct CyclicHook {
    generation: u8,
    executions: Arc<Mutex<Vec<u8>>>,
    rollback_order: Arc<Mutex<Vec<u8>>>,
}

impl CommitHook for CyclicHook {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.executions.lock().unwrap().push(self.generation);
        // Not `.expect(..)`: past the generation bound this returns `Err`
        // instead, which is fine to ignore here — `commit()` itself is what
        // surfaces the failure.
        let _ = op.add_commit_hook(CyclicHook {
            generation: self.generation + 1,
            executions: self.executions.clone(),
            rollback_order: self.rollback_order.clone(),
        });
        PreCommitRet::ok(self, op)
    }

    fn on_rollback(self) {
        self.rollback_order.lock().unwrap().push(self.generation);
    }
}

#[tokio::test]
async fn reentrant_registration_cycle_is_bounded_and_rolls_back() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executions = Arc::new(Mutex::new(Vec::new()));
    let rollback_order = Arc::new(Mutex::new(Vec::new()));

    op.add_commit_hook(CyclicHook {
        generation: 0,
        executions: executions.clone(),
        rollback_order: rollback_order.clone(),
    })
    .unwrap();

    let result = op.commit().await;
    assert!(
        result.is_err(),
        "an unbroken re-registration cycle must fail loudly instead of hanging"
    );

    // Generations 0..=MAX_HOOK_GENERATIONS all executed (each stages the
    // next); the one that would exceed the bound is rejected at registration
    // time and never gets a chance to run.
    let expected_generations: Vec<u8> = (0..=MAX_HOOK_GENERATIONS).collect();
    assert_eq!(*executions.lock().unwrap(), expected_generations);
    // Every generation that did execute is notified of the rollback, in the
    // same (registration/execution) order.
    assert_eq!(*rollback_order.lock().unwrap(), expected_generations);

    Ok(())
}

// ===========================================================================
// runs_after ordering tests
// ===========================================================================
//
// `runs_after` matches on `TypeId`, so each test hook kind below is its own
// struct (HookA / HookB / HookC) rather than a shared parameterized type —
// `deps` is built at construction time (`vec![TypeId::of::<HookA>()]`), never
// relied on as a `const`. `pre_commit` pushes `"pre:X"`, `post_commit` pushes
// `"post:X"`, `on_rollback` pushes `"rollback:X"` onto a shared log.

#[derive(Debug)]
struct HookA {
    log: Arc<Mutex<Vec<&'static str>>>,
    deps: Vec<TypeId>,
}

impl CommitHook for HookA {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.log.lock().unwrap().push("pre:A");
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.log.lock().unwrap().push("post:A");
    }

    fn on_rollback(self) {
        self.log.lock().unwrap().push("rollback:A");
    }

    fn runs_after(&self) -> &[TypeId] {
        &self.deps
    }
}

#[derive(Debug)]
struct HookB {
    log: Arc<Mutex<Vec<&'static str>>>,
    deps: Vec<TypeId>,
}

impl CommitHook for HookB {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.log.lock().unwrap().push("pre:B");
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.log.lock().unwrap().push("post:B");
    }

    fn on_rollback(self) {
        self.log.lock().unwrap().push("rollback:B");
    }

    fn runs_after(&self) -> &[TypeId] {
        &self.deps
    }
}

#[derive(Debug)]
struct HookC {
    log: Arc<Mutex<Vec<&'static str>>>,
    deps: Vec<TypeId>,
}

impl CommitHook for HookC {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.log.lock().unwrap().push("pre:C");
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.log.lock().unwrap().push("post:C");
    }

    fn on_rollback(self) {
        self.log.lock().unwrap().push("rollback:C");
    }

    fn runs_after(&self) -> &[TypeId] {
        &self.deps
    }
}

#[tokio::test]
async fn runs_after_defers_until_dependency_executes() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));

    // B is registered first but declares runs_after A: it must defer until
    // A's still-pending instance executes, even though A registered later.
    op.add_commit_hook(HookB {
        log: log.clone(),
        deps: vec![TypeId::of::<HookA>()],
    })
    .unwrap();
    op.add_commit_hook(HookA {
        log: log.clone(),
        deps: vec![],
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["pre:A", "pre:B", "post:A", "post:B"]
    );

    Ok(())
}

#[tokio::test]
async fn runs_after_vacuous_when_dependency_absent() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));

    // B declares runs_after A, but A is never registered on this operation —
    // the dependency imposes no constraint and registration order holds.
    op.add_commit_hook(HookB {
        log: log.clone(),
        deps: vec![TypeId::of::<HookA>()],
    })
    .unwrap();
    op.add_commit_hook(HookC {
        log: log.clone(),
        deps: vec![],
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["pre:B", "pre:C", "post:B", "post:C"]
    );

    Ok(())
}

#[tokio::test]
async fn runs_after_dependency_already_executed_does_not_defer() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));

    // A registers (and executes) first; B's dependency is already satisfied
    // by the time B reaches the front of the queue, so no deferral occurs.
    op.add_commit_hook(HookA {
        log: log.clone(),
        deps: vec![],
    })
    .unwrap();
    op.add_commit_hook(HookB {
        log: log.clone(),
        deps: vec![TypeId::of::<HookA>()],
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(
        *log.lock().unwrap(),
        vec!["pre:A", "pre:B", "post:A", "post:B"]
    );

    Ok(())
}

/// Mergeable hook that declares `runs_after` a `Producer`. Records the event
/// list it saw at each `pre_commit` call so the test can assert it ran
/// exactly once (merged) rather than as two separate generations.
#[derive(Debug)]
struct Consumer {
    events: Vec<&'static str>,
    deps: Vec<TypeId>,
    pre_commit_calls: Arc<Mutex<Vec<Vec<&'static str>>>>,
}

impl CommitHook for Consumer {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.pre_commit_calls
            .lock()
            .unwrap()
            .push(self.events.clone());
        PreCommitRet::ok(self, op)
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.events.append(&mut other.events);
        true
    }

    fn runs_after(&self) -> &[TypeId] {
        &self.deps
    }
}

/// Stages a fresh `Consumer` from its own `pre_commit` — the obix cross-outbox
/// repost shape this design exists for.
#[derive(Debug)]
struct Producer {
    log: Arc<Mutex<Vec<&'static str>>>,
    consumer_deps: Vec<TypeId>,
    consumer_calls: Arc<Mutex<Vec<Vec<&'static str>>>>,
}

impl CommitHook for Producer {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.log.lock().unwrap().push("pre:Producer");
        op.add_commit_hook(Consumer {
            events: vec!["staged"],
            deps: self.consumer_deps.clone(),
            pre_commit_calls: self.consumer_calls.clone(),
        })
        .expect("a real commit pass must accept re-entrant registration");
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn reentrant_stage_merges_into_deferred_pending_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));
    let consumer_calls = Arc::new(Mutex::new(Vec::new()));
    let consumer_deps = vec![TypeId::of::<Producer>()];

    // Consumer registers FIRST (deps=[Producer]) and so defers; Producer
    // registers second and, from its own pre_commit, re-entrantly stages
    // another Consumer instance. Because the original Consumer is deferred
    // (not yet executed), it is still a merge target: the staged instance
    // folds into it instead of starting a fresh generation.
    op.add_commit_hook(Consumer {
        events: vec!["own"],
        deps: consumer_deps.clone(),
        pre_commit_calls: consumer_calls.clone(),
    })
    .unwrap();
    op.add_commit_hook(Producer {
        log: log.clone(),
        consumer_deps,
        consumer_calls: consumer_calls.clone(),
    })
    .unwrap();

    op.commit().await?;

    assert_eq!(*log.lock().unwrap(), vec!["pre:Producer"]);

    let calls = consumer_calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "Consumer's pre_commit must run exactly once — merged into the \
         deferred pending instance, not a fresh generation"
    );
    assert_eq!(calls[0], vec!["own", "staged"]);

    Ok(())
}

#[tokio::test]
async fn runs_after_cycle_errors_loudly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));

    // C has no dependency and runs to completion. A and B mutually depend on
    // each other and can never both be satisfied.
    op.add_commit_hook(HookC {
        log: log.clone(),
        deps: vec![],
    })
    .unwrap();
    op.add_commit_hook(HookA {
        log: log.clone(),
        deps: vec![TypeId::of::<HookB>()],
    })
    .unwrap();
    op.add_commit_hook(HookB {
        log: log.clone(),
        deps: vec![TypeId::of::<HookA>()],
    })
    .unwrap();

    let result = op.commit().await;
    match result {
        Err(sqlx::Error::Protocol(msg)) => {
            assert!(
                msg.contains("cycle"),
                "error message should mention the cycle, got: {msg}"
            );
        }
        other => panic!("expected sqlx::Error::Protocol, got: {other:?}"),
    }

    // C's pre_commit had already completed, so it is notified of the
    // rollback. A and B never ran their pre_commit (they were only ever
    // popped for the (failed) deferral check), so they log nothing at all —
    // no pre, no rollback, per the existing on_rollback contract.
    assert_eq!(*log.lock().unwrap(), vec!["pre:C", "rollback:C"]);

    Ok(())
}

#[tokio::test]
async fn runs_after_chain_orders_transitively() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let log = Arc::new(Mutex::new(Vec::new()));

    // C -> B -> A, registered in reverse dependency order.
    op.add_commit_hook(HookC {
        log: log.clone(),
        deps: vec![TypeId::of::<HookB>()],
    })
    .unwrap();
    op.add_commit_hook(HookB {
        log: log.clone(),
        deps: vec![TypeId::of::<HookA>()],
    })
    .unwrap();
    op.add_commit_hook(HookA {
        log: log.clone(),
        deps: vec![],
    })
    .unwrap();

    op.commit().await?;

    let pre: Vec<&'static str> = log
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| entry.starts_with("pre:"))
        .copied()
        .collect();
    assert_eq!(pre, vec!["pre:A", "pre:B", "pre:C"]);

    Ok(())
}
