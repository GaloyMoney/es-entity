mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp,
    hooks::{PreCommitHook, PreCommitHookWithData},
};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn basic_hook_execution_with_db_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();

    let hook = PreCommitHook::new(move |_op| {
        let executed = executed_clone.clone();
        Box::pin(async move {
            // sqlx::query!("SELECT NOW()")
            //     .fetch_one(op.as_executor())
            //     .await?;
            *executed.lock().unwrap() = true;
            Ok(())
        })
    });

    op.add_pre_commit_hook("hook", hook);

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
    let result_clone = result.clone();

    let hook = PreCommitHookWithData::new(
        move |_op, data: String| {
            let result = result_clone.clone();
            Box::pin(async move {
                *result.lock().unwrap() = data;
                Ok(())
            })
        },
        "test_data".to_string(),
        |_a, b| b, // Replace merge strategy
    );

    op.add_pre_commit_hook("hook_1", hook);

    op.commit().await?;

    assert_eq!(*result.lock().unwrap(), "test_data");

    Ok(())
}
