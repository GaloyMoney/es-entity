use std::{any::Any, collections::HashMap, future::Future, pin::Pin};

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

// --- Type-erased storage ---

type ErasedExecutor = Box<
    dyn for<'c> FnOnce(HookOperation<'c>) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send,
>;

type ErasedExecutorWithData = Box<
    dyn for<'c> FnOnce(
            HookOperation<'c>,
            Box<dyn Any + Send>,
        ) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send,
>;

type ErasedMerger =
    Box<dyn Fn(Box<dyn Any + Send>, Box<dyn Any + Send>) -> Box<dyn Any + Send> + Send>;

enum HookStorage {
    Individual(Vec<ErasedExecutor>),
    Merged {
        data: Box<dyn Any + Send>,
        executor: ErasedExecutorWithData,
        merger: ErasedMerger,
    },
}

// --- PreCommitHooks ---

pub struct PreCommitHooks {
    hooks: HashMap<&'static str, HookStorage>,
}

impl PreCommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    fn add_individual(&mut self, hook_name: &'static str, executor: ErasedExecutor) {
        match self.hooks.get_mut(hook_name) {
            None => {
                self.hooks
                    .insert(hook_name, HookStorage::Individual(vec![executor]));
            }
            Some(HookStorage::Individual(executors)) => {
                executors.push(executor);
            }
            Some(HookStorage::Merged { .. }) => {
                panic!("hook key mismatch: expected individual hook, found merged");
            }
        }
    }

    fn add_merged(
        &mut self,
        hook_name: &'static str,
        executor: ErasedExecutorWithData,
        data: Box<dyn Any + Send>,
        merger: ErasedMerger,
    ) {
        match self.hooks.get_mut(&hook_name) {
            None => {
                self.hooks.insert(
                    hook_name,
                    HookStorage::Merged {
                        data,
                        executor,
                        merger,
                    },
                );
            }
            Some(HookStorage::Merged {
                data: existing,
                merger,
                ..
            }) => {
                let old = std::mem::replace(existing, Box::new(()));
                *existing = merger(old, data);
            }
            Some(HookStorage::Individual(_)) => {
                panic!("hook key mismatch: expected merged hook, found individual");
            }
        }
    }

    pub(super) async fn execute(
        mut self,
        op: &mut impl AtomicOperation,
    ) -> Result<(), sqlx::Error> {
        let mut op = HookOperation::new(op);

        for (_, storage) in self.hooks.drain() {
            match storage {
                HookStorage::Individual(executors) => {
                    for executor in executors {
                        op = executor(op).await?;
                    }
                }
                HookStorage::Merged { data, executor, .. } => {
                    op = executor(op, data).await?;
                }
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

// --- Hook types (use BoxFuture directly, no generic Fut) ---

pub struct PreCommitHook<H> {
    hook: H,
}

impl<H> PreCommitHook<H>
where
    H: for<'c> FnOnce(HookOperation<'c>) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send
        + 'static,
{
    pub fn new(hook: H) -> Self {
        Self { hook }
    }
}

pub struct PreCommitHookWithData<H, D, M> {
    hook: H,
    data: D,
    merge: M,
}

impl<H, D, M> PreCommitHookWithData<H, D, M>
where
    D: Send + 'static,
    H: for<'c> FnOnce(
            HookOperation<'c>,
            D,
        ) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send
        + 'static,
    M: Fn(D, D) -> D + Send + 'static,
{
    pub fn new(hook: H, data: D, merge: M) -> Self {
        Self { hook, data, merge }
    }
}

// --- IntoPreCommitHook trait ---

pub trait IntoPreCommitHook: Send {
    fn register(self, hook_name: &'static str, hooks: &mut PreCommitHooks);
}

impl<H> IntoPreCommitHook for PreCommitHook<H>
where
    H: for<'c> FnOnce(HookOperation<'c>) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send
        + 'static,
{
    fn register(self, hook_name: &'static str, hooks: &mut PreCommitHooks) {
        hooks.add_individual(hook_name, Box::new(self.hook));
    }
}

impl<H, D, M> IntoPreCommitHook for PreCommitHookWithData<H, D, M>
where
    D: Send + 'static,
    H: for<'c> FnOnce(
            HookOperation<'c>,
            D,
        ) -> BoxFuture<'c, Result<HookOperation<'c>, sqlx::Error>>
        + Send
        + 'static,
    M: Fn(D, D) -> D + Send + 'static,
{
    fn register(self, hook_name: &'static str, hooks: &mut PreCommitHooks) {
        let PreCommitHookWithData { hook, data, merge } = self;

        let executor: ErasedExecutorWithData = Box::new(move |conn, boxed_data| {
            let data = *boxed_data.downcast::<D>().unwrap();
            hook(conn, data)
        });
        let merger: ErasedMerger = Box::new(move |a, b| {
            let a = *a.downcast::<D>().unwrap();
            let b = *b.downcast::<D>().unwrap();
            Box::new(merge(a, b))
        });
        hooks.add_merged(hook_name, executor, Box::new(data), merger);
    }
}

pub mod merge {
    pub fn extend<T>(mut a: Vec<T>, b: Vec<T>) -> Vec<T> {
        a.extend(b);
        a
    }

    pub fn replace<T>(_: T, b: T) -> T {
        b
    }
}
