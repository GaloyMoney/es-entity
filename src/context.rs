use im::HashMap;
use serde::Serialize;

use std::{cell::RefCell, sync::Arc};

thread_local! {
    static CONTEXT_STACK: RefCell<EventContext> =
        RefCell::new(EventContext::new());
}

pub struct EventContext {
    data: Arc<HashMap<&'static str, serde_json::Value>>,
}

impl EventContext {
    fn new() -> Self {
        Self {
            data: Arc::new(HashMap::new()),
        }
    }

    pub fn current() -> Self {
        unimplemented!()
    }

    pub fn insert<T: Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        self.data = Arc::new(self.data.update(key, serde_json::to_value(value)?));
        Ok(())
    }

    pub fn as_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current() {
        let mut ctx = EventContext::current();
        let value = serde_json::json!({ "hello": "world" });
        ctx.insert("data", &value);
        assert_current(value);
    }

    fn assert_current(value: serde_json::Value) {
        let ctx = EventContext::current();

        assert_eq!(ctx.as_json().unwrap(), value);
    }
}
