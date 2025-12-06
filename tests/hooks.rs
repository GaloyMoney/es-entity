mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp,
    hooks::{HookOperation, PreCommitHook},
};
use std::sync::{Arc, Mutex};

// --- Hook definitions ---

struct ExecuteTracker(Arc<Mutex<bool>>);

impl PreCommitHook for ExecuteTracker {
    async fn execute(self, mut op: HookOperation<'_>) -> Result<HookOperation<'_>, sqlx::Error> {
        sqlx::query!("SELECT NOW()")
            .fetch_one(op.as_executor())
            .await?;
        *self.0.lock().unwrap() = true;
        Ok(op)
    }
}

struct DataCapture {
    data: String,
    result: Arc<Mutex<String>>,
}

impl PreCommitHook for DataCapture {
    async fn execute(self, op: HookOperation<'_>) -> Result<HookOperation<'_>, sqlx::Error> {
        *self.result.lock().unwrap() = self.data;
        Ok(op)
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.data = std::mem::take(&mut other.data);
        true
    }
}

struct MergeableEvents(Vec<String>, Arc<Mutex<Vec<String>>>);

impl PreCommitHook for MergeableEvents {
    async fn execute(self, op: HookOperation<'_>) -> Result<HookOperation<'_>, sqlx::Error> {
        *self.1.lock().unwrap() = self.0;
        Ok(op)
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        self.0.append(&mut other.0);
        true
    }
}

struct NonMergeableHook(Arc<Mutex<i32>>);

impl PreCommitHook for NonMergeableHook {
    async fn execute(self, op: HookOperation<'_>) -> Result<HookOperation<'_>, sqlx::Error> {
        *self.0.lock().unwrap() += 1;
        Ok(op)
    }
    // Default merge returns false - each executes separately
}

// --- Tests ---

#[tokio::test]
async fn basic_hook_execution_with_db_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));

    op.add_pre_commit_hook(ExecuteTracker(executed.clone()));

    assert!(
        !*executed.lock().unwrap(),
        "Hook should not execute before commit"
    );

    op.commit().await?;

    assert!(*executed.lock().unwrap(), "Hook should execute on commit");

    Ok(())
}

#[tokio::test]
async fn hook_with_data() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(String::new()));

    op.add_pre_commit_hook(DataCapture {
        data: "test_data".to_string(),
        result: result.clone(),
    });

    op.commit().await?;

    assert_eq!(*result.lock().unwrap(), "test_data");

    Ok(())
}

#[tokio::test]
async fn hooks_merge_when_returning_true() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(Vec::new()));

    op.add_pre_commit_hook(MergeableEvents(vec!["e1".into()], result.clone()));
    op.add_pre_commit_hook(MergeableEvents(
        vec!["e2".into(), "e3".into()],
        result.clone(),
    ));

    op.commit().await?;

    // Should be merged into single execution
    assert_eq!(*result.lock().unwrap(), vec!["e1", "e2", "e3"]);

    Ok(())
}

#[tokio::test]
async fn hooks_execute_separately_when_not_merged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let count = Arc::new(Mutex::new(0));

    op.add_pre_commit_hook(NonMergeableHook(count.clone()));
    op.add_pre_commit_hook(NonMergeableHook(count.clone()));
    op.add_pre_commit_hook(NonMergeableHook(count.clone()));

    op.commit().await?;

    // Each hook should execute separately
    assert_eq!(*count.lock().unwrap(), 3);

    Ok(())
}

#[tokio::test]
async fn hook_can_access_cached_time() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let op = DbOp::init(&pool).await?;
    let mut op = op.with_system_time();

    let captured_time = op.now();

    struct TimeCapture(Arc<Mutex<Option<chrono::DateTime<chrono::Utc>>>>);

    impl PreCommitHook for TimeCapture {
        async fn execute(self, op: HookOperation<'_>) -> Result<HookOperation<'_>, sqlx::Error> {
            *self.0.lock().unwrap() = op.now();
            Ok(op)
        }
    }

    let result = Arc::new(Mutex::new(None));
    op.add_pre_commit_hook(TimeCapture(result.clone()));

    op.commit().await?;

    assert_eq!(result.lock().unwrap().unwrap(), captured_time);

    Ok(())
}
