mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};
use std::sync::{Arc, Mutex};

// --- Pre-commit only hook ---

struct PreCommitTracker(Arc<Mutex<bool>>);

impl CommitHook for PreCommitTracker {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        sqlx::query!("SELECT NOW()")
            .fetch_one(op.as_executor())
            .await?;
        *self.0.lock().unwrap() = true;
        PreCommitRet::ok(self, op)
    }
}

// --- Post-commit only hook ---

struct PostCommitTracker(Arc<Mutex<bool>>);

impl CommitHook for PostCommitTracker {
    fn post_commit(self) {
        *self.0.lock().unwrap() = true;
    }
}

// --- Both pre and post commit ---

struct FullCommitHook {
    data: String,
    pre_result: Arc<Mutex<String>>,
    post_result: Arc<Mutex<String>>,
}

impl CommitHook for FullCommitHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        *self.pre_result.lock().unwrap() = self.data.clone();
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        *self.post_result.lock().unwrap() = format!("post:{}", self.data);
    }
}

// --- Mergeable hook ---

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

// --- Non-mergeable hook ---

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

// --- Tests ---

#[tokio::test]
async fn pre_commit_hook_executes_before_commit() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));
    op.add_commit_hook(PreCommitTracker(executed.clone()));

    assert!(!*executed.lock().unwrap());
    op.commit().await?;
    assert!(*executed.lock().unwrap());

    Ok(())
}

#[tokio::test]
async fn post_commit_hook_executes_after_commit() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));
    op.add_commit_hook(PostCommitTracker(executed.clone()));

    assert!(!*executed.lock().unwrap());
    op.commit().await?;
    assert!(*executed.lock().unwrap());

    Ok(())
}

#[tokio::test]
async fn both_pre_and_post_commit_execute() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let pre_result = Arc::new(Mutex::new(String::new()));
    let post_result = Arc::new(Mutex::new(String::new()));

    op.add_commit_hook(FullCommitHook {
        data: "test".to_string(),
        pre_result: pre_result.clone(),
        post_result: post_result.clone(),
    });

    op.commit().await?;

    assert_eq!(*pre_result.lock().unwrap(), "test");
    assert_eq!(*post_result.lock().unwrap(), "post:test");

    Ok(())
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
    });
    op.add_commit_hook(MergeableEvents {
        events: vec!["e2".into(), "e3".into()],
        pre_result: pre_result.clone(),
        post_result: post_result.clone(),
    });

    op.commit().await?;

    assert_eq!(*pre_result.lock().unwrap(), vec!["e1", "e2", "e3"]);
    assert_eq!(*post_result.lock().unwrap(), vec!["e1", "e2", "e3"]);

    Ok(())
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
    });
    op.add_commit_hook(NonMergeableHook {
        pre_count: pre_count.clone(),
        post_count: post_count.clone(),
    });
    op.add_commit_hook(NonMergeableHook {
        pre_count: pre_count.clone(),
        post_count: post_count.clone(),
    });

    op.commit().await?;

    assert_eq!(*pre_count.lock().unwrap(), 3);
    assert_eq!(*post_count.lock().unwrap(), 3);

    Ok(())
}

#[tokio::test]
async fn hook_can_access_cached_time() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let op = DbOp::init(&pool).await?;
    let mut op = op.with_system_time();

    let captured_time = op.now();

    struct TimeCapture(Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>);

    impl CommitHook for TimeCapture {
        async fn pre_commit(
            self,
            op: HookOperation<'_>,
        ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
            *self.0.lock().unwrap() = op.now();
            PreCommitRet::ok(self, op)
        }
    }

    let result = Arc::new(Mutex::new(None));
    op.add_commit_hook(TimeCapture(result.clone()));

    op.commit().await?;

    assert_eq!(result.lock().unwrap().unwrap(), captured_time);

    Ok(())
}
