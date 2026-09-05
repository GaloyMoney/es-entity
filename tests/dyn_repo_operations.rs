//! Pins the property the `_in_op` bound relaxation exists for: the derived
//! `EsRepo` `_in_op` methods must accept a mutable reference to a
//! `dyn AtomicOperation` **directly** — no `&mut &mut` double-reference
//! workaround. A caller that only holds `&mut dyn AtomicOperation` (e.g. a job
//! function relaxed to `impl AtomicOperation + ?Sized`) cannot manufacture a
//! concrete, `Sized` operation type to satisfy an unrelaxed bound, so this
//! must compile — and pass — with a single reference throughout.
//!
//! Every generated `_in_op` function is unconditionally `?Sized` — including
//! ones with a nested field or a `post_persist_hook` configured, which
//! internally reborrow through one more `&mut` at the boundary into a
//! Sized-only callee (a sibling repo's method, or a user hook) rather than
//! falling back to `Sized` themselves. `Users` here keeps neither, so it
//! exercises the plain, unwrapped path.

mod entities;
mod helpers;

use entities::user::*;
use es_entity::{AtomicOperation, DbOp, *};
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn generated_in_op_fns_accept_dyn_atomic_operation_directly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool.clone());
    let mut db_op = DbOp::init(&pool).await?;

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Dyn Op")
        .build()
        .unwrap();

    // `create_in_op` takes `op: &mut OP`. Passing a `&mut dyn AtomicOperation`
    // straight through unifies `OP = dyn AtomicOperation` — only possible
    // because the generated bound is `OP: AtomicOperation + ?Sized`.
    let op: &mut dyn AtomicOperation = &mut db_op;
    let mut user = users.create_in_op(op, new_user).await?;

    let _ = user.update_name("Dyn Op Updated");
    let op: &mut dyn AtomicOperation = &mut db_op;
    users.update_in_op(op, &mut user).await?;

    // The read-side `_in_op` methods ride on `IntoOneTimeExecutorAt`'s
    // `impl<'c, O> IntoOneTimeExecutorAt<'c> for &mut O where O: AtomicOperation
    // + ?Sized` — also directly callable with `&mut dyn AtomicOperation`.
    let op: &mut dyn AtomicOperation = &mut db_op;
    let found = users.find_by_id_in_op(op, user.id).await?;
    assert_eq!(found.name, "Dyn Op Updated");

    let op: &mut dyn AtomicOperation = &mut db_op;
    let all: std::collections::HashMap<UserId, User> = users.find_all_in_op(op, &[user.id]).await?;
    assert!(all.contains_key(&user.id));

    db_op.commit().await?;
    Ok(())
}

// --- The harder case: a `post_persist_hook` configured -------------------
//
// `execute_post_persist_hook` forwards `op` into this hook method, whose
// signature — like every existing hook in this codebase and downstream —
// is generic over a plain (implicitly `Sized`) `impl AtomicOperation`. The
// generated wrapper reborrows through one more `&mut` before calling it
// (see `post_persist_hook.rs`), so the hook method itself needs no changes
// and `create_in_op` still accepts `&mut dyn AtomicOperation` directly.

#[derive(Debug)]
pub struct DummyHookError;

impl std::fmt::Display for DummyHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dummy hook error")
    }
}

impl std::error::Error for DummyHookError {}

// A second, distinct entity (`Task`) — `#[derive(EsRepo)]` generates
// companion types named after the entity (`UserCreateError`, ...), so a
// second repo over `User` in this same file would collide with `Users`'.
use entities::task::*;

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Task",
    columns(status(ty = "String", create(accessor = "status"))),
    post_persist_hook(method = "on_persist", error = "DummyHookError")
)]
pub struct TasksWithHook {
    pool: PgPool,
    hook_calls: std::sync::atomic::AtomicUsize,
}

impl TasksWithHook {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            hook_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    // Deliberately the plain, `Sized`-only shape every existing hook method
    // uses — this must keep compiling unmodified.
    async fn on_persist<OP: es_entity::AtomicOperation>(
        &self,
        _op: &mut OP,
        _entity: &Task,
        _new_events: es_entity::LastPersisted<'_, TaskEvent>,
    ) -> Result<(), DummyHookError> {
        self.hook_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn hook_configured_repo_still_accepts_dyn_atomic_operation_directly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = TasksWithHook::new(pool.clone());
    let mut db_op = DbOp::init(&pool).await?;

    let new_task = NewTask::builder()
        .id(TaskId::new())
        .status("open")
        .build()
        .unwrap();

    let op: &mut dyn AtomicOperation = &mut db_op;
    let _task = tasks.create_in_op(op, new_task).await?;

    db_op.commit().await?;
    assert_eq!(
        tasks.hook_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    Ok(())
}
