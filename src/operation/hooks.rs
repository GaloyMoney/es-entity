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
    inner: &'c mut sqlx::PgConnection,
}
impl<'c, T: AtomicOperation> From<&'c mut T> for HookOperation<'c> {
    fn from(op: &'c mut T) -> Self {
        Self {
            now: op.now(),
            inner: op.as_executor(),
        }
    }
}

impl<'c> AtomicOperation for HookOperation<'c> {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.inner
    }
}

type ErasedExecutor = Box<
    dyn for<'a> FnOnce(
            &mut HookOperation<'a>,
            Box<dyn Any + Send>,
        ) -> BoxFuture<'a, Result<(), sqlx::Error>>
        + Send,
>;

type ErasedMerger =
    Box<dyn Fn(Box<dyn Any + Send>, Box<dyn Any + Send>) -> Box<dyn Any + Send> + Send>;

struct HookEntry {
    data: Box<dyn Any + Send>,
    executor: ErasedExecutor,
    merger: Option<ErasedMerger>,
}

pub struct PreCommitHooks {
    hooks: HashMap<TypeId, HookEntry>,
}

impl PreCommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    pub fn add<K, H, D, Fut, M>(&mut self, hook: H, data: Option<D>, merge: Option<M>)
    where
        K: 'static,
        D: Send + 'static,
        H: FnOnce(&mut HookOperation, D) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
        M: Fn(D, D) -> D + Send + 'static,
    {
        let type_id = TypeId::of::<K>();

        if let Some(entry) = self.hooks.get_mut(&type_id) {
            if let Some(ref merger) = entry.merger {
                let existing = std::mem::replace(&mut entry.data, Box::new(()));
                entry.data = merger(existing, Box::new(data));
            } else {
                // No merge strategy - replace
                entry.data = Box::new(data);
                entry.executor = Box::new(move |op, boxed_data| {
                    let data = *boxed_data.downcast::<D>().unwrap();
                    Box::pin(hook(op, data))
                });
            }
        } else {
            let merger: Option<ErasedMerger> = merge.map(|m| {
                Box::new(
                    move |a: Box<dyn Any + Send>, b: Box<dyn Any + Send>| -> Box<dyn Any + Send> {
                        let a = *a.downcast::<D>().unwrap();
                        let b = *b.downcast::<D>().unwrap();
                        Box::new(m(a, b))
                    },
                ) as ErasedMerger
            });

            let entry = HookEntry {
                data: Box::new(data),
                executor: Box::new(move |op, boxed_data| {
                    let data = *boxed_data.downcast::<D>().unwrap();
                    Box::pin(hook(op, data))
                }),
                merger,
            };
            self.hooks.insert(type_id, entry);
        }
    }

    pub async fn execute(mut self, conn: &mut impl AtomicOperation) -> Result<(), sqlx::Error> {
        let mut hook_op = HookOperation::from(conn);

        for (_, entry) in self.hooks.drain() {
            let HookEntry { data, executor, .. } = entry;
            executor(&mut hook_op, data).await?;
        }
        Ok(())
    }
}
