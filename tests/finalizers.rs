use es_entity::{
    AtomicOperation, DbOp,
    hooks::{CommitHook, HookOperation, PreCommitRet},
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Vec<&'static str>>>);
impl Probe {
    fn push(&self, label: &'static str) {
        self.0.lock().unwrap().push(label);
    }
    fn labels(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}
struct Seal(Probe);
impl CommitHook for Seal {
    fn is_finalizer(&self) -> bool {
        true
    }
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.0.push("seal");
        PreCommitRet::ok(self, op)
    }
    fn post_commit(self) {
        self.0.push("committed");
    }
    fn on_rollback(self) {
        self.0.push("rolled_back");
    }
}
struct Spawn {
    probe: Probe,
    depth: u8,
}
impl CommitHook for Spawn {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        self.probe.push("ordinary");
        if self.depth > 0 {
            assert!(
                op.add_commit_hook(Spawn {
                    probe: self.probe.clone(),
                    depth: self.depth - 1
                })
                .is_ok()
            );
        }
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn finalizers_wait_for_all_reentrant_ordinary_generations() -> anyhow::Result<()> {
    let pool = sqlx::PgPool::connect(&std::env::var("PG_CON")?).await?;
    let probe = Probe::default();
    let mut op = DbOp::init(&pool).await?;
    assert!(op.add_commit_hook(Seal(probe.clone())).is_ok());
    assert!(
        op.add_commit_hook(Spawn {
            probe: probe.clone(),
            depth: 2
        })
        .is_ok()
    );
    op.commit().await?;
    assert_eq!(
        probe.labels(),
        ["ordinary", "ordinary", "ordinary", "seal", "committed"]
    );
    Ok(())
}

struct BadSeal(Probe);
impl CommitHook for BadSeal {
    fn is_finalizer(&self) -> bool {
        true
    }
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        assert!(
            op.add_commit_hook(Spawn {
                probe: self.0.clone(),
                depth: 0
            })
            .is_ok()
        );
        PreCommitRet::ok(self, op)
    }
}

#[tokio::test]
async fn finalizers_cannot_reopen_publication() -> anyhow::Result<()> {
    let pool = sqlx::PgPool::connect(&std::env::var("PG_CON")?).await?;
    let probe = Probe::default();
    let mut op = DbOp::init(&pool).await?;
    assert!(op.add_commit_hook(Seal(probe.clone())).is_ok());
    assert!(op.add_commit_hook(BadSeal(probe.clone())).is_ok());
    let error = op
        .commit()
        .await
        .expect_err("late hook registration must abort");
    assert!(error.to_string().contains("finalizers cannot register"));
    assert_eq!(probe.labels(), ["seal", "rolled_back"]);
    Ok(())
}
