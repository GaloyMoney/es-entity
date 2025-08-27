use std::sync::atomic::{AtomicU64, Ordering};

static CONTEXT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContextId(u64);

impl ContextId {
    pub fn next() -> Self {
        let id = CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(id)
    }
}
