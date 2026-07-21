mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp, OpWithTime,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};
use std::sync::{Arc, Mutex};

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
