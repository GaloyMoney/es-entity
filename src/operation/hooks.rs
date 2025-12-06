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
    pub(super) fn new(op: &'c mut impl AtomicOperation) -> Self {
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

// --- User-facing trait ---

pub trait PreCommitHook: Send + 'static + Sized {
    fn execute(
        self,
        op: HookOperation<'_>,
    ) -> impl Future<Output = Result<HookOperation<'_>, sqlx::Error>> + Send;

    /// Try to merge `other` into `self`.
    /// Returns true if merged (other will be dropped).
    /// Returns false if not merged (both will execute separately).
    /// Default returns false.
    fn merge(&mut self, other: &mut Self) -> bool {
        let _ = other;
        false
    }
}

// --- Object-safe internal trait ---

trait DynHook: Send {
    fn execute_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>;

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<H: PreCommitHook> DynHook for H {
    fn execute_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>> {
        Box::pin((*self).execute(op))
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

pub struct PreCommitHooks {
    hooks: HashMap<TypeId, Vec<Box<dyn DynHook>>>,
}

impl PreCommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    pub(super) fn add<H: PreCommitHook>(&mut self, hook: H) {
        let type_id = TypeId::of::<H>();
        let hooks_vec = self.hooks.entry(type_id).or_default();

        let mut new_hook: Box<dyn DynHook> = Box::new(hook);

        // Try to merge with the last existing hook of this type
        if let Some(last) = hooks_vec.last_mut() {
            if last.try_merge(new_hook.as_mut()) {
                return; // Merged successfully, new_hook is dropped
            }
        }

        // Not merged (or no existing hooks), add as separate entry
        hooks_vec.push(new_hook);
    }

    pub(super) async fn execute(
        mut self,
        op: &mut impl AtomicOperation,
    ) -> Result<(), sqlx::Error> {
        let mut op = HookOperation::new(op);

        for (_, hooks_vec) in self.hooks.drain() {
            for hook in hooks_vec {
                op = hook.execute_boxed(op).await?;
            }
        }
        Ok(())
    }
}

impl Default for PreCommitHooks {
    fn default() -> Self {
        Self::new()
    }
}
