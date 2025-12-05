use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    pin::Pin,
};

use super::AtomicOperation;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// --- HookOperation ---

pub struct HookOperation {
    now: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) tx: sqlx::PgTransaction<'static>,
}

impl HookOperation {
    pub(super) fn new(
        tx: sqlx::PgTransaction<'static>,
        now: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self { now, tx }
    }
}

impl AtomicOperation for HookOperation {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        &mut *self.tx
    }
}

// --- Type-erased storage ---

type ErasedExecutor =
    Box<dyn FnOnce(&mut HookOperation) -> BoxFuture<'_, Result<(), sqlx::Error>> + Send>;

type ErasedExecutorWithData = Box<
    dyn FnOnce(&mut HookOperation, Box<dyn Any + Send>) -> BoxFuture<'_, Result<(), sqlx::Error>>
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

pub(super) struct PreCommitHooks {
    hooks: HashMap<TypeId, HookStorage>,
}

impl PreCommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    fn add_individual<K: 'static>(&mut self, executor: ErasedExecutor) {
        let type_id = TypeId::of::<K>();

        match self.hooks.get_mut(&type_id) {
            None => {
                self.hooks
                    .insert(type_id, HookStorage::Individual(vec![executor]));
            }
            Some(HookStorage::Individual(executors)) => {
                executors.push(executor);
            }
            Some(HookStorage::Merged { .. }) => {
                panic!("hook key mismatch: expected individual hook, found merged");
            }
        }
    }

    fn add_merged<K: 'static>(
        &mut self,
        executor: ErasedExecutorWithData,
        data: Box<dyn Any + Send>,
        merger: ErasedMerger,
    ) {
        let type_id = TypeId::of::<K>();

        match self.hooks.get_mut(&type_id) {
            None => {
                self.hooks.insert(
                    type_id,
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

    pub async fn execute(mut self, mut op: &mut HookOperation) -> Result<(), sqlx::Error> {
        for (_, storage) in self.hooks.drain() {
            match storage {
                HookStorage::Individual(executors) => {
                    for executor in executors {
                        executor(&mut op).await?;
                    }
                }
                HookStorage::Merged { data, executor, .. } => {
                    executor(&mut op, data).await?;
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

// --- Hook types ---

pub struct PreCommitHook<H> {
    hook: H,
}

impl<H, Fut> PreCommitHook<H>
where
    H: FnOnce(&mut HookOperation) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
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

impl<H, D, M, Fut> PreCommitHookWithData<H, D, M>
where
    D: Send + 'static,
    H: FnOnce(&mut HookOperation, D) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
    M: Fn(D, D) -> D + Send + 'static,
{
    pub fn new(hook: H, data: D, merge: M) -> Self {
        Self { hook, data, merge }
    }
}

// --- IntoPreCommitHook trait ---

pub trait IntoPreCommitHook {
    fn register<K: 'static>(self, hooks: &mut PreCommitHooks);
}

impl<H, Fut> IntoPreCommitHook for PreCommitHook<H>
where
    H: FnOnce(&mut HookOperation) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
{
    fn register<K: 'static>(self, hooks: &mut PreCommitHooks) {
        let executor: ErasedExecutor = Box::new(move |op| Box::pin((self.hook)(op)));
        hooks.add_individual::<K>(executor);
    }
}

impl<H, D, M, Fut> IntoPreCommitHook for PreCommitHookWithData<H, D, M>
where
    D: Send + 'static,
    H: FnOnce(&mut HookOperation, D) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
    M: Fn(D, D) -> D + Send + 'static,
{
    fn register<K: 'static>(self, hooks: &mut PreCommitHooks) {
        let executor: ErasedExecutorWithData = Box::new(move |op, boxed_data| {
            let data = *boxed_data.downcast::<D>().unwrap();
            Box::pin((self.hook)(op, data))
        });
        let merger: ErasedMerger = Box::new(move |a, b| {
            let a = *a.downcast::<D>().unwrap();
            let b = *b.downcast::<D>().unwrap();
            Box::new((self.merge)(a, b))
        });
        hooks.add_merged::<K>(executor, Box::new(self.data), merger);
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
