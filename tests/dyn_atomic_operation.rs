mod helpers;

use es_entity::operation::{
    AtomicOperation, DbOp, SavepointOperation,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};
use std::{
    any::TypeId,
    sync::{Arc, Mutex},
};

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

/// The pin: if `AtomicOperation` ever regains a method that is generic
/// without `Self: Sized`, this stops compiling with E0038.
fn assert_object_safe(op: &mut dyn AtomicOperation) -> &mut dyn AtomicOperation {
    op
}

#[tokio::test]
async fn dyn_atomic_operation_drives_hooks_and_savepoints() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let post = Arc::new(Mutex::new(Vec::new()));
    let mut db_op = DbOp::init(&pool).await?;

    {
        let op: &mut dyn AtomicOperation = assert_object_safe(&mut db_op);

        assert!(op.supports_hooks());
        let _ = op.maybe_now();
        let _ = op.clock();
        let _ = op.connection();

        let hook = CountingHook {
            label: "root".to_string(),
            post: post.clone(),
        };
        op.add_commit_hook_dyn(TypeId::of::<CountingHook>(), Box::new(hook))
            .ok()
            .expect("dyn op must accept a hook");

        let registered = op
            .commit_hook_dyn(TypeId::of::<CountingHook>())
            .and_then(|h| h.as_any().downcast_ref::<CountingHook>())
            .expect("the hook just registered must be readable back through the dyn op");
        assert_eq!(registered.label, "root");

        // Savepoints work uniformly through the trait object, too.
        let result = op
            .with_savepoint(async |sp| {
                let sp_op: &mut dyn AtomicOperation = sp;
                assert!(sp_op.supports_hooks());
                sp_op
                    .add_commit_hook_dyn(
                        TypeId::of::<CountingHook>(),
                        Box::new(CountingHook {
                            label: "savepoint".to_string(),
                            post: post.clone(),
                        }),
                    )
                    .ok()
                    .expect("savepoint must accept a hook via the dyn op");
                Ok::<_, sqlx::Error>(())
            })
            .await?;
        assert!(result.is_ok());
    }

    db_op.commit().await?;

    let mut ran = post.lock().unwrap().clone();
    ran.sort();
    assert_eq!(ran, vec!["root".to_string(), "savepoint".to_string()]);

    Ok(())
}

#[tokio::test]
async fn dyn_atomic_operation_generic_wrapper_still_works() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let post = Arc::new(Mutex::new(Vec::new()));
    let mut db_op = DbOp::init(&pool).await?;

    // The Sized-only generic wrappers are unaffected on a concrete (non-dyn)
    // operation: they route through the same erased methods internally.
    db_op
        .add_commit_hook(CountingHook {
            label: "typed".to_string(),
            post: post.clone(),
        })
        .unwrap();
    assert_eq!(db_op.commit_hook::<CountingHook>().unwrap().label, "typed");

    db_op.commit().await?;
    assert_eq!(post.lock().unwrap().clone(), vec!["typed".to_string()]);

    Ok(())
}
