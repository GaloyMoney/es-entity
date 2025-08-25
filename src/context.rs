use im::HashMap;
use serde::Serialize;

use std::{cell::RefCell, marker::PhantomData, rc::Rc};

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

thread_local! {
    static CONTEXT_STACK: RefCell<Vec<ContextData>> = RefCell::new(Vec::new());
}

pub struct EventContext {
    data: ContextData,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for EventContext {
    fn drop(&mut self) {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if !stack.is_empty() {
                stack.pop();
            }
        });
    }
}

impl EventContext {
    fn new(data: ContextData) -> Self {
        Self {
            data,
            _not_send: PhantomData,
        }
    }

    pub fn current() -> Self {
        let data = CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if stack.is_empty() {
                stack.push(ContextData::new());
            } else {
                let current_data = stack.last().unwrap().clone();
                stack.push(current_data);
            }
            stack.last().unwrap().clone()
        });
        EventContext::new(data)
    }

    pub fn seed(data: ContextData) -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            stack.push(data.clone());
        });
        EventContext::new(data)
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
        self.data = self.data.update(key, json_value);

        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if let Some(last) = stack.last_mut() {
                *last = self.data.clone();
            }
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

    #[test]
    fn current() {
        let mut ctx = EventContext::current();
        let value = serde_json::json!({ "hello": "world" });
        ctx.insert("data", &value).unwrap();
        inner(&value);
        assert_current(serde_json::json!({ "data": value }));
    }

    fn inner(value: &serde_json::Value) {
        let mut ctx = EventContext::current();
        ctx.insert("new_data", &value).unwrap();
        assert_current(serde_json::json!({ "data": value, "new_data": value}));
    }

    fn assert_current(expected: serde_json::Value) {
        let ctx = EventContext::current();
        assert_eq!(ctx.as_json().unwrap(), expected);
    }

    #[test]
    fn thread_isolation() {
        let mut ctx = EventContext::current();
        let value = serde_json::json!({ "main": "thread" });
        ctx.insert("data", &value).unwrap();

        let ctx_data = ctx.data().clone();
        let handle = std::thread::spawn(move || {
            {
                let mut ctx = EventContext::seed(ctx_data);
                ctx.insert("thread", &serde_json::json!("local")).unwrap();
                assert_current(
                    serde_json::json!({ "data": { "main": "thread" }, "thread": "local" }),
                );
            }
            CONTEXT_STACK.with(|c| {
                assert!(
                    c.borrow().is_empty(),
                    "Thread stack should be empty after context drops"
                );
            });
        });

        assert_current(serde_json::json!({ "data": value }));
        handle.join().unwrap();

        assert_current(serde_json::json!({ "data": value }));
    }
}
