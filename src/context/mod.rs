mod with_es_context;

use im::HashMap;
use serde::Serialize;

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

pub use with_es_context::*;

#[derive(Debug, Clone)]
pub struct ContextData(HashMap<&'static str, serde_json::Value>);

impl ContextData {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn update(&self, key: &'static str, value: serde_json::Value) -> Self {
        Self(self.0.update(key, value))
    }

    fn iter(&self) -> impl Iterator<Item = (&'static str, &serde_json::Value)> {
        self.0.iter().map(|(&k, v)| (k, v))
    }
}

struct StackEntry {
    id: Weak<()>,
    data: ContextData,
}

thread_local! {
    static CONTEXT_STACK: RefCell<Vec<StackEntry>> = RefCell::new(Vec::new());
}

pub struct EventContext {
    id: Rc<()>,
}

impl Drop for EventContext {
    fn drop(&mut self) {
        if Rc::strong_count(&self.id) == 1 {
            CONTEXT_STACK.with(|c| {
                let mut stack = c.borrow_mut();
                stack.retain(|entry| {
                    if let Some(strong_id) = entry.id.upgrade() {
                        !Rc::ptr_eq(&strong_id, &self.id)
                    } else {
                        false
                    }
                });
            });
        }
    }
}

impl EventContext {
    pub fn current() -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for i in (0..stack.len()).rev() {
                if let Some(strong_id) = stack[i].id.upgrade() {
                    return EventContext { id: strong_id };
                }
            }

            let id = Rc::new(());
            let data = ContextData::new();
            stack.push(StackEntry {
                id: Rc::downgrade(&id),
                data,
            });

            EventContext { id }
        })
    }

    pub fn seed(data: ContextData) -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            let id = Rc::new(());
            stack.push(StackEntry {
                id: Rc::downgrade(&id),
                data,
            });

            EventContext { id }
        })
    }

    pub fn insert<T: Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let json_value = serde_json::to_value(value)?;

        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for entry in stack.iter_mut().rev() {
                if let Some(strong_id) = entry.id.upgrade() {
                    if Rc::ptr_eq(&strong_id, &self.id) {
                        entry.data = entry.data.update(key, json_value);
                        return;
                    }
                }
            }
            panic!("EventContext missing on CONTEXT_STACK")
        });

        Ok(())
    }

    pub fn data(&self) -> ContextData {
        CONTEXT_STACK.with(|c| {
            let stack = c.borrow();
            for entry in stack.iter().rev() {
                if let Some(strong_id) = entry.id.upgrade() {
                    if Rc::ptr_eq(&strong_id, &self.id) {
                        return entry.data.clone();
                    }
                }
            }
            panic!("EventContext missing on CONTEXT_STACK")
        })
    }

    pub fn as_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        let data = self.data();
        let mut map = serde_json::Map::new();
        for (k, v) in data.iter() {
            map.insert(k.to_string(), v.clone());
        }
        Ok(serde_json::Value::Object(map))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack_depth() -> usize {
        CONTEXT_STACK.with(|c| c.borrow().len())
    }

    fn current_json() -> serde_json::Value {
        EventContext::current().as_json().unwrap()
    }

    #[test]
    fn assert_stack_depth() {
        fn assert_inner() {
            let _ctx = EventContext::current();
            assert_eq!(stack_depth(), 1);
        }
        assert_eq!(stack_depth(), 0);
        {
            let _ctx = EventContext::current();
            assert_eq!(stack_depth(), 1);
            assert_inner();
        }
        assert_eq!(stack_depth(), 0);
    }

    #[test]
    fn insert() {
        fn insert_inner(value: &serde_json::Value) {
            let mut ctx = EventContext::current();
            ctx.insert("new_data", &value).unwrap();
            assert_eq!(
                current_json(),
                serde_json::json!({ "data": value, "new_data": value})
            );
            assert_eq!(stack_depth(), 1);
        }

        let mut ctx = EventContext::current();
        assert_eq!(current_json(), serde_json::json!({}));
        let value = serde_json::json!({ "hello": "world" });
        ctx.insert("data", &value).unwrap();
        assert_eq!(stack_depth(), 1);
        assert_eq!(current_json(), serde_json::json!({ "data": value }));
        insert_inner(&value);
        assert_eq!(stack_depth(), 1);
        assert_eq!(
            current_json(),
            serde_json::json!({ "data": value, "new_data": value})
        );
        assert_eq!(stack_depth(), 1);
        let new_value = serde_json::json!({ "hello": "new_world" });
        ctx.insert("data", &new_value).unwrap();
        assert_eq!(
            current_json(),
            serde_json::json!({ "data": new_value, "new_data": value})
        );
    }

    #[test]
    fn thread_isolation() {
        let mut ctx = EventContext::current();
        let value = serde_json::json!({ "main": "thread" });
        ctx.insert("data", &value).unwrap();
        assert_eq!(stack_depth(), 1);

        let ctx_data = ctx.data();
        let handle = std::thread::spawn(move || {
            assert_eq!(stack_depth(), 0);
            let mut ctx = EventContext::seed(ctx_data);
            assert_eq!(stack_depth(), 1);
            ctx.insert("thread", &serde_json::json!("local")).unwrap();
            assert_eq!(
                current_json(),
                serde_json::json!({ "data": { "main": "thread" }, "thread": "local" }),
            );
        });

        assert_eq!(current_json(), serde_json::json!({ "data": value }));
        handle.join().unwrap();

        assert_eq!(current_json(), serde_json::json!({ "data": value }));
    }

    #[tokio::test]
    async fn async_context() {
        async fn inner_async() {
            let mut ctx = EventContext::current();
            ctx.insert("async_inner", &serde_json::json!("value"))
                .unwrap();
            assert_eq!(
                current_json(),
                serde_json::json!({ "async_data": { "test": "async" }, "async_inner": "value" })
            );
        }

        let mut ctx = EventContext::current();
        assert_eq!(current_json(), serde_json::json!({}));

        let value = serde_json::json!({ "test": "async" });
        ctx.insert("async_data", &value).unwrap();
        assert_eq!(current_json(), serde_json::json!({ "async_data": value }));

        inner_async().await;

        assert_eq!(
            current_json(),
            serde_json::json!({ "async_data": value, "async_inner": "value" })
        );
    }

    #[tokio::test]
    async fn with_event_context_spawned() {
        let mut ctx = EventContext::current();
        ctx.insert("parent", &serde_json::json!("context")).unwrap();

        let handle = tokio::spawn(
            async {
                assert_eq!(stack_depth(), 2);

                EventContext::current()
                    .insert("spawned", &serde_json::json!("value"))
                    .unwrap();

                assert_eq!(
                    current_json(),
                    serde_json::json!({ "parent": "context", "spawned": "value" })
                );
                tokio::task::yield_now().await;
                current_json()
            }
            .with_event_context(ctx.data()),
        );

        let result = handle.await.unwrap();
        assert_eq!(
            result,
            serde_json::json!({ "parent": "context", "spawned": "value" })
        );

        assert_eq!(current_json(), serde_json::json!({ "parent": "context" }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_event_context_spawned_multi_thread() {
        let mut ctx = EventContext::current();
        ctx.insert("parent", &serde_json::json!("context")).unwrap();

        let handle = tokio::spawn(
            async {
                assert_eq!(stack_depth(), 1);

                EventContext::current()
                    .insert("spawned", &serde_json::json!("value"))
                    .unwrap();

                assert_eq!(
                    current_json(),
                    serde_json::json!({ "parent": "context", "spawned": "value" })
                );
                let data = EventContext::current().data();
                tokio::task::yield_now().with_event_context(data).await;
                current_json()
            }
            .with_event_context(ctx.data()),
        );

        let result = handle.await.unwrap();
        assert_eq!(
            result,
            serde_json::json!({ "parent": "context", "spawned": "value" })
        );

        assert_eq!(current_json(), serde_json::json!({ "parent": "context" }));
    }
}
