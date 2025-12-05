mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp,
    hooks::{PreCommitHook, PreCommitHookWithData},
};
use std::sync::{Arc, Mutex};

// Hook key types for different tests
struct HookKey1;
struct HookKey2;
struct HookKey3;

#[tokio::test]
async fn basic_hook_execution_with_db_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();

    let hook = PreCommitHook::new(move |op| {
        let executed = executed_clone.clone();
        async move {
            sqlx::query!("SELECT NOW()")
                .fetch_one(op.as_executor())
                .await?;
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });

    op.add_pre_commit_hook::<HookKey1>(hook);

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
            async move {
                *result.lock().unwrap() = data;
                Ok(())
            }
        },
        "test_data".to_string(),
        |_a, b| b, // Replace merge strategy
    );

    op.add_pre_commit_hook::<HookKey1>(hook);

    op.commit().await?;

    assert_eq!(*result.lock().unwrap(), "test_data");

    Ok(())
}

#[tokio::test]
async fn multiple_individual_hooks_same_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let execution_order = Arc::new(Mutex::new(Vec::new()));

    // Add multiple hooks with the same key - they should all execute
    for i in 1..=3 {
        let order_clone = execution_order.clone();
        let hook = PreCommitHook::new(move |_op| {
            let order = order_clone.clone();
            async move {
                order.lock().unwrap().push(i);
                Ok(())
            }
        });
        op.add_pre_commit_hook::<HookKey1>(hook);
    }

    op.commit().await?;

    let order = execution_order.lock().unwrap();
    assert_eq!(order.len(), 3, "All three hooks should execute");
    assert_eq!(*order, vec![1, 2, 3], "Hooks should execute in order");

    Ok(())
}

#[tokio::test]
async fn multiple_hooks_different_keys() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let execution_order = Arc::new(Mutex::new(Vec::new()));

    let order_clone1 = execution_order.clone();
    op.add_pre_commit_hook::<HookKey1>(PreCommitHook::new(move |_op| {
        let order = order_clone1.clone();
        async move {
            order.lock().unwrap().push(1);
            Ok(())
        }
    }));

    let order_clone2 = execution_order.clone();
    op.add_pre_commit_hook::<HookKey2>(PreCommitHook::new(move |_op| {
        let order = order_clone2.clone();
        async move {
            order.lock().unwrap().push(2);
            Ok(())
        }
    }));

    let order_clone3 = execution_order.clone();
    op.add_pre_commit_hook::<HookKey3>(PreCommitHook::new(move |_op| {
        let order = order_clone3.clone();
        async move {
            order.lock().unwrap().push(3);
            Ok(())
        }
    }));

    op.commit().await?;

    let order = execution_order.lock().unwrap();
    assert_eq!(order.len(), 3, "All three hooks should execute");
    assert!(order.contains(&1));
    assert!(order.contains(&2));
    assert!(order.contains(&3));

    Ok(())
}

#[tokio::test]
async fn hook_merging_with_data() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(0));
    let result_clone = result.clone();

    // Add first hook with data = 5
    let hook1 = PreCommitHookWithData::new(
        move |_op, data: i32| {
            let result = result_clone.clone();
            async move {
                *result.lock().unwrap() = data;
                Ok(())
            }
        },
        5,
        |a: i32, b: i32| a + b, // Merge function adds the values
    );
    op.add_pre_commit_hook::<HookKey1>(hook1);

    // Add second hook with same key and data = 10
    // The merge function should be called with (5, 10)
    let hook2 = PreCommitHookWithData::new(
        |_op, data: i32| async move {
            unreachable!("Executor should use merged data: {}", data);
        },
        10,
        |a: i32, b: i32| a + b,
    );
    op.add_pre_commit_hook::<HookKey1>(hook2);

    op.commit().await?;

    assert_eq!(
        *result.lock().unwrap(),
        15,
        "Data should be merged: 5 + 10 = 15"
    );

    Ok(())
}

#[tokio::test]
async fn hook_merging_with_vec_extend() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(Vec::new()));
    let result_clone = result.clone();

    // Use the provided extend merge function
    use es_entity::operation::hooks::merge::extend;

    let hook1 = PreCommitHookWithData::new(
        move |_op, data: Vec<i32>| {
            let result = result_clone.clone();
            async move {
                *result.lock().unwrap() = data;
                Ok(())
            }
        },
        vec![1, 2, 3],
        extend,
    );
    op.add_pre_commit_hook::<HookKey1>(hook1);

    let hook2 = PreCommitHookWithData::new(
        |_op, _data: Vec<i32>| async move { unreachable!("Should use merged data") },
        vec![4, 5, 6],
        extend,
    );
    op.add_pre_commit_hook::<HookKey1>(hook2);

    op.commit().await?;

    assert_eq!(
        *result.lock().unwrap(),
        vec![1, 2, 3, 4, 5, 6],
        "Vectors should be merged via extend"
    );

    Ok(())
}

#[tokio::test]
async fn hook_merging_with_replace() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(String::new()));
    let result_clone = result.clone();

    use es_entity::operation::hooks::merge::replace;

    let hook1 = PreCommitHookWithData::new(
        move |_op, data: String| {
            let result = result_clone.clone();
            async move {
                *result.lock().unwrap() = data;
                Ok(())
            }
        },
        "first".to_string(),
        replace,
    );
    op.add_pre_commit_hook::<HookKey1>(hook1);

    let hook2 = PreCommitHookWithData::new(
        |_op, _data: String| async move { unreachable!("Should use merged data") },
        "second".to_string(),
        replace,
    );
    op.add_pre_commit_hook::<HookKey1>(hook2);

    op.commit().await?;

    assert_eq!(
        *result.lock().unwrap(),
        "second",
        "Second value should replace first"
    );

    Ok(())
}

#[tokio::test]
async fn hooks_preserved_through_with_time_transition() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();

    // Add hook before transitioning to DbOpWithTime
    let hook1 = PreCommitHook::new(move |_op| {
        let executed = executed_clone.clone();
        async move {
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });
    op.add_pre_commit_hook::<HookKey1>(hook1);

    // Transition to DbOpWithTime
    let mut op = op.with_system_time();

    // Add another hook after transition
    let executed2 = Arc::new(Mutex::new(false));
    let executed2_clone = executed2.clone();

    let hook2 = PreCommitHook::new(move |_op| {
        let executed = executed2_clone.clone();
        async move {
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });
    op.add_pre_commit_hook::<HookKey2>(hook2);

    op.commit().await?;

    assert!(
        *executed.lock().unwrap(),
        "Hook added before transition should execute"
    );
    assert!(
        *executed2.lock().unwrap(),
        "Hook added after transition should execute"
    );

    Ok(())
}

#[tokio::test]
async fn nested_transaction_with_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut outer_op = DbOp::init(&pool).await?;

    let outer_executed = Arc::new(Mutex::new(false));
    let outer_executed_clone = outer_executed.clone();

    // Add hook to outer transaction
    let hook1 = PreCommitHook::new(move |_op| {
        let executed = outer_executed_clone.clone();
        async move {
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });
    outer_op.add_pre_commit_hook::<HookKey1>(hook1);

    // Begin nested transaction
    let mut inner_op = outer_op.begin().await?;

    let inner_executed = Arc::new(Mutex::new(false));
    let inner_executed_clone = inner_executed.clone();

    // Add hook to inner transaction
    let hook2 = PreCommitHook::new(move |_op| {
        let executed = inner_executed_clone.clone();
        async move {
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });
    inner_op.add_pre_commit_hook::<HookKey2>(hook2);

    // Commit inner transaction (should execute inner hooks)
    inner_op.commit().await?;
    assert!(*inner_executed.lock().unwrap(), "Inner hook should execute");

    // Commit outer transaction (should execute outer hooks)
    outer_op.commit().await?;
    assert!(*outer_executed.lock().unwrap(), "Outer hook should execute");

    Ok(())
}

#[tokio::test]
async fn hook_can_access_cached_time() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let op = DbOp::init(&pool).await?;
    let mut op = op.with_system_time();

    let captured_time = op.now();
    let result = Arc::new(Mutex::new(None));
    let result_clone = result.clone();

    let hook = PreCommitHook::new(move |op| {
        let result = result_clone.clone();
        let now = op.now(); // Capture the value before moving into async block
        async move {
            *result.lock().unwrap() = now;
            Ok(())
        }
    });
    op.add_pre_commit_hook::<HookKey1>(hook);

    op.commit().await?;

    let hook_time = result.lock().unwrap().unwrap();
    assert_eq!(
        hook_time, captured_time,
        "Hook should see the same cached time"
    );

    Ok(())
}

#[tokio::test]
async fn transaction_does_not_support_hooks() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?;

    let executed = Arc::new(Mutex::new(false));
    let executed_clone = executed.clone();

    let hook = PreCommitHook::new(move |_op| {
        let executed = executed_clone.clone();
        async move {
            *executed.lock().unwrap() = true;
            Ok(())
        }
    });

    // Raw transaction should return false for add_pre_commit_hook
    let supported = tx.add_pre_commit_hook::<HookKey1>(hook);
    assert!(!supported, "Raw transaction should not support hooks");

    tx.commit().await?;

    assert!(
        !*executed.lock().unwrap(),
        "Hook should not execute on raw transaction"
    );

    Ok(())
}

#[tokio::test]
async fn multiple_data_merges() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut op = DbOp::init(&pool).await?;

    let result = Arc::new(Mutex::new(0));
    let result_clone = result.clone();

    // Add three hooks with the same key - all should merge
    for value in [5, 10, 15] {
        let result_for_hook = result_clone.clone();
        let hook = PreCommitHookWithData::new(
            move |_op, data: i32| {
                let result = result_for_hook.clone();
                async move {
                    *result.lock().unwrap() = data;
                    Ok(())
                }
            },
            value,
            |a: i32, b: i32| a + b,
        );
        op.add_pre_commit_hook::<HookKey1>(hook);
    }

    op.commit().await?;

    assert_eq!(
        *result.lock().unwrap(),
        30,
        "All values should merge: 5 + 10 + 15 = 30"
    );

    Ok(())
}
