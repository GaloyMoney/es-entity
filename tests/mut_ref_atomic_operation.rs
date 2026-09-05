mod helpers;

use es_entity::{
    clock::ClockHandle,
    operation::{
        AtomicOperation, DbOp, SavepointOperation,
        hooks::{CommitHook, HookOperation, PreCommitRet},
    },
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct CountingHook {
    label: String,
    post: Arc<Mutex<Vec<String>>>,
}

impl CommitHook for CountingHook {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        self.post.lock().unwrap().push(self.label);
    }
}

async fn touch(op: &mut impl AtomicOperation) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT 1 as one")
        .fetch_one(op.as_executor())
        .await?;
    Ok(())
}

#[tokio::test]
async fn generic_callee_accepts_a_mut_borrow() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut db_op = DbOp::init(&pool).await?;

    touch(&mut &mut db_op).await?;

    let result = db_op
        .with_savepoint(async |mut sp| touch(&mut sp).await)
        .await?;
    assert!(result.is_ok());

    db_op.commit().await?;
    Ok(())
}

#[tokio::test]
async fn clock_and_cached_time_forward_through_mut_ref() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let (clock, _ctrl) = ClockHandle::manual();
    let mut db_op = DbOp::init_with_clock(&pool, &clock).await?;

    let inner_maybe_now = db_op.maybe_now();
    assert!(
        inner_maybe_now.is_some(),
        "a manual clock must cache the time on init"
    );
    let inner_clock_now = db_op.clock().now();

    fn read_clock_state(
        op: &mut impl AtomicOperation,
    ) -> (
        Option<chrono::DateTime<chrono::Utc>>,
        chrono::DateTime<chrono::Utc>,
    ) {
        (op.maybe_now(), op.clock().now())
    }
    let (borrowed_maybe_now, borrowed_clock_now) = read_clock_state(&mut &mut db_op);

    assert_eq!(borrowed_maybe_now, inner_maybe_now);
    assert_eq!(borrowed_clock_now, inner_clock_now);

    db_op.commit().await?;
    Ok(())
}

#[tokio::test]
async fn hooks_register_through_mut_ref() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let post = Arc::new(Mutex::new(Vec::new()));
    let mut db_op = DbOp::init(&pool).await?;

    fn register(op: &mut impl AtomicOperation, hook: CountingHook) -> Result<(), CountingHook> {
        op.add_commit_hook(hook)
    }
    register(
        &mut &mut db_op,
        CountingHook {
            label: "via-borrow".to_string(),
            post: post.clone(),
        },
    )
    .expect("the borrow must forward hook registration, not fall to the Err default");

    assert_eq!(
        db_op.commit_hook::<CountingHook>().unwrap().label,
        "via-borrow"
    );

    db_op.commit().await?;
    assert_eq!(post.lock().unwrap().clone(), vec!["via-borrow".to_string()]);
    Ok(())
}

#[tokio::test]
async fn savepoint_through_mut_ref_keeps_hook_support() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let post = Arc::new(Mutex::new(Vec::new()));
    let mut db_op = DbOp::init(&pool).await?;

    async fn isolate_and_hook(
        op: &mut impl AtomicOperation,
        hook: CountingHook,
    ) -> Result<Result<(), sqlx::Error>, sqlx::Error> {
        op.with_savepoint(async |sp| {
            sp.add_commit_hook(hook)
                .expect("savepoint reached through the borrow must accept a hook");
            Ok::<_, sqlx::Error>(())
        })
        .await
    }

    let result = isolate_and_hook(
        &mut &mut db_op,
        CountingHook {
            label: "savepoint".to_string(),
            post: post.clone(),
        },
    )
    .await?;
    assert!(result.is_ok());

    db_op.commit().await?;
    assert_eq!(post.lock().unwrap().clone(), vec!["savepoint".to_string()]);
    Ok(())
}

#[tokio::test]
async fn mut_ref_to_dyn_atomic_operation_is_itself_an_operation() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let post = Arc::new(Mutex::new(Vec::new()));
    let mut db_op = DbOp::init(&pool).await?;

    {
        let mut dynamic: &mut dyn AtomicOperation = &mut db_op;
        touch(&mut dynamic).await?;

        fn register(op: &mut impl AtomicOperation, hook: CountingHook) -> Result<(), CountingHook> {
            op.add_commit_hook(hook)
        }
        register(
            &mut dynamic,
            CountingHook {
                label: "via-dyn".to_string(),
                post: post.clone(),
            },
        )
        .expect("add_commit_hook must still reach a &mut dyn AtomicOperation");
    }

    db_op.commit().await?;
    assert_eq!(post.lock().unwrap().clone(), vec!["via-dyn".to_string()]);
    Ok(())
}
