use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    pin::Pin,
};

use super::AtomicOperation;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct HookOperation<'c> {
    now: Option<chrono::DateTime<chrono::Utc>>,
    conn: &'c mut sqlx::PgConnection,
}

impl<'c> HookOperation<'c> {
    fn new(op: &'c mut impl AtomicOperation) -> Self {
        Self {
            now: op.now(),
            conn: op.as_executor(),
        }
    }
}

impl<'c> AtomicOperation for HookOperation<'c> {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.conn
    }
}

// --- Pre-commit result type ---

pub struct PreCommitRet<'c, H> {
    op: HookOperation<'c>,
    hook: H,
}

impl<'c, H> PreCommitRet<'c, H> {
    pub fn ok(hook: H, op: HookOperation<'c>) -> Result<Self, sqlx::Error> {
        Ok(Self { op, hook })
    }
}

// --- User-facing trait ---

pub trait CommitHook: Send + 'static + Sized {
    /// Called before the transaction commits. Can perform database operations.
    /// Returns Self so it can be used in post_commit.
    fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> impl Future<Output = Result<PreCommitRet<'_, Self>, sqlx::Error>> + Send {
        async { PreCommitRet::ok(self, op) }
    }

    /// Called after the transaction has successfully committed.
    /// Cannot fail, not async.
    fn post_commit(self) {
        // Default: do nothing
    }

    /// Try to merge `other` into `self`.
    /// Returns true if merged (other will be dropped).
    /// Returns false if not merged (both will execute separately).
    fn merge(&mut self, _other: &mut Self) -> bool {
        false
    }

    /// Execute the hook immediately.
    /// Useful when `add_commit_hook` returns false (hooks not supported).
    fn force_execute_pre_commit(
        self,
        op: &mut impl AtomicOperation,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send {
        async {
            let hook_op = HookOperation::new(op);
            self.pre_commit(hook_op).await?;
            Ok(())
        }
    }
}

// --- Object-safe internal trait ---

trait DynHook: Send {
    fn pre_commit_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<(HookOperation<'c>, Box<dyn DynHook>), sqlx::Error>>;

    fn post_commit_boxed(self: Box<Self>);

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<H: CommitHook> DynHook for H {
    fn pre_commit_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<(HookOperation<'c>, Box<dyn DynHook>), sqlx::Error>> {
        Box::pin(async move {
            let ret = (*self).pre_commit(op).await?;
            Ok((ret.op, Box::new(ret.hook) as Box<dyn DynHook>))
        })
    }

    fn post_commit_boxed(self: Box<Self>) {
        (*self).post_commit()
    }

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool {
        let other_h = other
            .as_any_mut()
            .downcast_mut::<H>()
            .expect("hook type mismatch");
        self.merge(other_h)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// --- Storage ---

pub struct CommitHooks {
    hooks: HashMap<TypeId, Vec<Box<dyn DynHook>>>,
}

impl CommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    pub(super) fn add<H: CommitHook>(&mut self, hook: H) {
        let type_id = TypeId::of::<H>();
        let hooks_vec = self.hooks.entry(type_id).or_default();

        let mut new_hook: Box<dyn DynHook> = Box::new(hook);

        if let Some(last) = hooks_vec.last_mut() {
            if last.try_merge(new_hook.as_mut()) {
                return;
            }
        }

        hooks_vec.push(new_hook);
    }

    pub(super) async fn execute_pre(
        mut self,
        op: &mut impl AtomicOperation,
    ) -> Result<PostCommitHooks, sqlx::Error> {
        let mut op = HookOperation::new(op);
        let mut post_hooks = Vec::new();

        for (_, hooks_vec) in self.hooks.drain() {
            for hook in hooks_vec {
                let (new_op, hook) = hook.pre_commit_boxed(op).await?;
                op = new_op;
                post_hooks.push(hook);
            }
        }

        Ok(PostCommitHooks { hooks: post_hooks })
    }
}

impl Default for CommitHooks {
    fn default() -> Self {
        Self::new()
    }
}

// --- Post-commit execution ---

pub struct PostCommitHooks {
    hooks: Vec<Box<dyn DynHook>>,
}

impl PostCommitHooks {
    pub(super) fn execute(self) {
        for hook in self.hooks {
            hook.post_commit_boxed();
        }
    }
}
