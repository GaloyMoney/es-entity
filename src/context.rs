use im::HashMap;
use serde::Serialize;

use std::{cell::RefCell, sync::Arc};

type ContextData = Arc<HashMap<&'static str, serde_json::Value>>;

thread_local! {
    static CONTEXT_STACK: RefCell<Vec<ContextData>> = RefCell::new(Vec::new());
}

pub struct EventContext {
    // Marker to track when to pop from stack
    _marker: (),
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
    pub fn current() -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if stack.is_empty() {
                stack.push(Arc::new(HashMap::new()));
            } else {
                let current_data = Arc::clone(stack.last().unwrap());
                stack.push(current_data);
            }
        });
        EventContext { _marker: () }
    }

    pub fn fork() -> Self {
        Self::current()
    }

    pub fn insert<T: Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let json_value = serde_json::to_value(value)?;
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if let Some(last) = stack.last_mut() {
                let new_data = last.update(key, json_value);
                *last = Arc::new(new_data);
            }
        });
        Ok(())
    }

    pub fn as_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        CONTEXT_STACK.with(|c| {
            let stack = c.borrow();
            if let Some(data) = stack.last() {
                let mut map = serde_json::Map::new();
                for (k, v) in data.iter() {
                    map.insert(k.to_string(), v.clone());
                }
                Ok(serde_json::Value::Object(map))
            } else {
                Ok(serde_json::Value::Object(serde_json::Map::new()))
            }
        })
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
        let mut ctx = EventContext::fork();
        ctx.insert("new_data", &value).unwrap();
        assert_current(serde_json::json!({ "data": value, "new_data": value}));
    }

    fn assert_current(expected: serde_json::Value) {
        let ctx = EventContext::current();
        assert_eq!(ctx.as_json().unwrap(), expected);
    }
}
