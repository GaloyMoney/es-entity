mod id;

use im::HashMap;
use serde::Serialize;

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use id::ContextId;

#[derive(Debug, Clone)]
pub struct ContextData {
    id: ContextId,
    data: HashMap<&'static str, serde_json::Value>,
}

impl ContextData {
    fn new(id: ContextId) -> Self {
        Self {
            id,
            data: HashMap::new(),
        }
    }

    fn update(&self, key: &'static str, value: serde_json::Value) -> Self {
        Self {
            id: self.id,
            data: self.data.update(key, value),
        }
    }

    fn iter(&self) -> impl Iterator<Item = (&'static str, &serde_json::Value)> {
        self.data.iter().map(|(&k, v)| (k, v))
    }
}

struct StackEntry {
    id: Weak<ContextId>,
    data: ContextData,
}

thread_local! {
    static CONTEXT_STACK: RefCell<Vec<StackEntry>> = RefCell::new(Vec::new());
}

pub struct EventContext {
    id: Rc<ContextId>,
    data: ContextData,
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

            // Try to find an existing context from the end
            for i in (0..stack.len()).rev() {
                if let Some(strong_id) = stack[i].id.upgrade() {
                    // Found a valid context, return it
                    return EventContext {
                        id: strong_id,
                        data: stack[i].data.clone(),
                    };
                }
            }

            // No valid context found, create a new one
            let id = Rc::new(ContextId::next());
            let data = ContextData::new(*id);
            stack.push(StackEntry {
                id: Rc::downgrade(&id),
                data: data.clone(),
            });

            EventContext { id, data }
        })
    }

    pub fn seed(data: ContextData) -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for entry in stack.iter() {
                if let Some(strong_id) = entry.id.upgrade() {
                    if *strong_id == data.id {
                        return EventContext {
                            id: strong_id,
                            data: entry.data.clone(),
                        };
                    }
                }
            }
            let id = Rc::new(data.id);
            stack.push(StackEntry {
                id: Rc::downgrade(&id),
                data: data.clone(),
            });

            EventContext { id, data }
        })
    }

    pub fn data(&self) -> &ContextData {
        &self.data
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
                        self.data = entry.data.update(key, json_value);
                        entry.data = self.data.clone();
                        return;
                    }
                }
            }
            self.data = self.data.update(key, json_value);
            stack.push(StackEntry {
                id: Rc::downgrade(&self.id),
                data: self.data.clone(),
            });
        });

        Ok(())
    }

    pub fn as_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut map = serde_json::Map::new();
        for (k, v) in self.data.iter() {
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

        let ctx_data = ctx.data().clone();
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
}
