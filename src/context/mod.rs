//! Thread-local system for adding context data to persisted events.
//!
//! This module provides a context propagation system for event sourcing that allows
//! attaching metadata (like request IDs, user IDs, or audit information) to events
//! as they are created and persisted to the database.
//!
//! # Core Components
//!
//! - [`EventContext`]: Thread-local context manager (`!Send`) that maintains a stack
//!   of contexts within a single thread
//! - [`ContextData`]: Immutable, thread-safe (`Send`) snapshot of context data that
//!   can be passed across thread boundaries
//! - [`WithEventContext`]: Extension trait for `Future` types to propagate context
//!   across async boundaries
//!
//! # Usage Patterns
//!
//! ## Same Thread Context
//! ```rust
//! use es_entity::context::EventContext;
//!
//! let mut ctx = EventContext::current();
//! ctx.insert("request_id", &"req-123").unwrap();
//!
//! // Fork for isolated scope
//! {
//!     let mut child = EventContext::fork();
//!     child.insert("operation", &"update").unwrap();
//!     // Both request_id and operation are available
//! }
//! // Only request_id remains in parent
//! ```
//!
//! ## Async Task Context
//! ```rust
//! use es_entity::context::{EventContext, WithEventContext};
//!
//! async fn spawn_with_context() {
//!     let mut ctx = EventContext::current();
//!     ctx.insert("user_id", &"user-456").unwrap();
//!
//!     let data = ctx.data();
//!     tokio::spawn(async move {
//!         // Context is available in spawned task
//!         let ctx = EventContext::current();
//!         // Has user_id from parent
//!     }.with_event_context(data)).await.unwrap();
//! }
//! ```
//!
//! ## Cross-Thread Context
//! ```rust
//! use es_entity::context::EventContext;
//!
//! let mut ctx = EventContext::current();
//! ctx.insert("trace_id", &"trace-789").unwrap();
//! let data = ctx.data();
//!
//! std::thread::spawn(move || {
//!     let ctx = EventContext::seed(data);
//!     // New thread has trace_id
//! });
//! ```
//!
//! # Database Integration
//!
//! When events are persisted using repositories with `event_context = true`, the current
//! context is automatically serialized to JSON and stored in a `context` column
//! alongside the event data, enabling comprehensive audit trails and debugging.

mod sqlx;
mod tracing;
mod with_event_context;

use serde::{Deserialize, Serialize};

use std::{borrow::Cow, cell::RefCell, rc::Rc, sync::Arc};

pub use tracing::*;
pub use with_event_context::*;

/// Immutable context data that can be safely shared across thread boundaries.
///
/// This struct holds key-value pairs of context information that gets attached
/// to events when they are persisted. It uses a copy-on-write entry vector
/// internally: cloning is a single atomic refcount bump, and mutation only
/// clones the (tiny) entry vector when the data is currently shared. Context
/// maps hold a handful of entries in practice, so a linear-scan vector is
/// cheaper than a hashed or persistent map on every operation that matters
/// (clone, insert, lookup).
///
/// `ContextData` is `Send` and can be passed between threads, unlike [`EventContext`]
/// which is thread-local. This makes it suitable for transferring context across
/// async boundaries via the [`WithEventContext`] trait.
#[derive(Debug, Clone)]
pub struct ContextData(Arc<Vec<(Cow<'static, str>, serde_json::Value)>>);

impl ContextData {
    fn new() -> Self {
        Self(Arc::new(Vec::new()))
    }

    fn insert(&mut self, key: &'static str, value: serde_json::Value) {
        let entries = Arc::make_mut(&mut self.0);
        if let Some((_, existing)) = entries.iter_mut().find(|(k, _)| *k == key) {
            *existing = value;
        } else {
            entries.push((Cow::Borrowed(key), value));
        }
    }

    /// Number of key-value pairs stored in this context.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the context holds no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(feature = "tracing-context")]
    pub(crate) fn with_tracing_info(mut self) -> Self {
        let tracing = TracingContext::current();
        self.insert(
            "tracing",
            serde_json::to_value(&tracing).expect("Could not inject tracing"),
        );
        self
    }

    pub fn lookup<T: serde::de::DeserializeOwned>(
        &self,
        key: &'static str,
    ) -> Result<Option<T>, serde_json::Error> {
        let Some((_, val)) = self.0.iter().find(|(k, _)| *k == key) else {
            return Ok(None);
        };
        serde_json::from_value(val.clone()).map(Some)
    }
}

impl Serialize for ContextData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in self.0.iter() {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ContextData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ContextDataVisitor;

        impl<'de> serde::de::Visitor<'de> for ContextDataVisitor {
            type Value = ContextData;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a map of context keys to JSON values")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::with_capacity(access.size_hint().unwrap_or(0));
                while let Some((key, value)) =
                    access.next_entry::<Cow<'static, str>, serde_json::Value>()?
                {
                    if let Some((_, existing)) = entries.iter_mut().find(|(k, _)| *k == key) {
                        *existing = value;
                    } else {
                        entries.push((key, value));
                    }
                }
                Ok(ContextData(Arc::new(entries)))
            }
        }

        deserializer.deserialize_map(ContextDataVisitor)
    }
}

struct StackEntry {
    id: Rc<()>,
    data: ContextData,
    /// Set by [`EventContext::insert`]. Lets [`EventContext::data_if_dirty`]
    /// skip the write-back clone when a poll left the context untouched.
    dirty: bool,
}

thread_local! {
    static CONTEXT_STACK: RefCell<Vec<StackEntry>> = const { RefCell::new(Vec::new()) };
}

/// Thread-local event context for tracking metadata throughout event sourcing operations.
///
/// `EventContext` provides a way to attach contextual information (like request IDs, audit info,
/// or operation metadata) to events as they are created and persisted. The context is managed
/// as a thread-local stack, allowing for nested contexts within the same thread.
///
/// # Thread Safety
///
/// This struct is deliberately `!Send` to ensure thread-local safety. It uses `Rc` for reference
/// counting which is not thread-safe. For propagating context across async boundaries or threads,
/// use the [`WithEventContext`] trait which safely transfers context data.
///
/// # Usage Patterns
///
/// - **Same thread**: Use [`fork()`](Self::fork) to create isolated child contexts
/// - **Async tasks**: Use [`with_event_context()`](WithEventContext::with_event_context) from the [`WithEventContext`] trait
/// - **New threads**: Use [`seed()`](Self::seed) with data from [`data()`](Self::data) to transfer context
///
/// # Examples
///
/// ```rust
/// use es_entity::context::EventContext;
///
/// // Create or get current context
/// let mut ctx = EventContext::current();
/// ctx.insert("user_id", &"123").unwrap();
///
/// // Fork for isolated scope
/// {
///     let mut child = EventContext::fork();
///     child.insert("operation", &"update").unwrap();
///     // Both user_id and operation are available here
/// }
/// // Only user_id remains in parent context
/// ```
pub struct EventContext {
    id: Rc<()>,
}

impl Drop for EventContext {
    fn drop(&mut self) {
        // If strong_count is 2, it means this EventContext + one StackEntry reference
        if Rc::strong_count(&self.id) == 2 {
            CONTEXT_STACK.with(|c| {
                let mut stack = c.borrow_mut();
                for i in (0..stack.len()).rev() {
                    if Rc::ptr_eq(&stack[i].id, &self.id) {
                        stack.remove(i);
                        break;
                    }
                }
            });
        }
    }
}

impl EventContext {
    /// Gets the current event context or creates a new one if none exists.
    ///
    /// This function is thread-local and will return a handle to the topmost context
    /// on the current thread's context stack. If no context exists, it will create
    /// a new empty context and push it onto the stack.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::EventContext;
    ///
    /// let ctx = EventContext::current();
    /// // Context is now available for the current thread
    /// ```
    pub fn current() -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            if let Some(last) = stack.last() {
                return EventContext {
                    id: last.id.clone(),
                };
            }

            let id = Rc::new(());
            let data = ContextData::new();
            stack.push(StackEntry {
                id: id.clone(),
                data,
                dirty: false,
            });

            EventContext { id }
        })
    }

    /// Creates a new event context seeded with the provided data.
    ///
    /// This creates a completely new context stack entry with the given context data,
    /// independent of any existing context. This is useful for starting fresh contexts
    /// in new threads or async tasks.
    ///
    /// # Arguments
    ///
    /// * `data` - The initial context data for the new context
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::{EventContext, ContextData};
    ///
    /// let data = EventContext::current().data();
    /// let new_ctx = EventContext::seed(data);
    /// // new_ctx now has its own independent context stack
    /// ```
    pub fn seed(data: ContextData) -> Self {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            let id = Rc::new(());
            stack.push(StackEntry {
                id: id.clone(),
                data,
                dirty: false,
            });

            EventContext { id }
        })
    }

    /// Creates a new isolated context that inherits data from the current context.
    ///
    /// This method creates a child context that starts with a copy of the current
    /// context's data. Changes made to the forked context will not affect the parent
    /// context, and when the forked context is dropped, the parent context remains
    /// unchanged. This is useful for creating isolated scopes within the same thread.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::EventContext;
    ///
    /// let mut parent = EventContext::current();
    /// parent.insert("shared", &"value").unwrap();
    ///
    /// {
    ///     let mut child = EventContext::fork();
    ///     child.insert("child_only", &"data").unwrap();
    ///     // child context has both "shared" and "child_only"
    /// }
    /// // parent context only has "shared" - "child_only" is gone
    /// ```
    pub fn fork() -> Self {
        let current = Self::current();
        let data = current.data();
        Self::seed(data)
    }

    /// Inserts a key-value pair into the current context.
    ///
    /// The value will be serialized to JSON and stored in the context data.
    /// This data will be available to all code running within this context
    /// and any child contexts created via `fork()`.
    ///
    /// # Arguments
    ///
    /// * `key` - A static string key to identify the value
    /// * `value` - Any serializable value to store in the context
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success or a `serde_json::Error` if serialization fails.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::EventContext;
    ///
    /// let mut ctx = EventContext::current();
    /// ctx.insert("user_id", &"12345").unwrap();
    /// ctx.insert("operation", &"transfer").unwrap();
    /// ```
    pub fn insert<T: Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let json_value = serde_json::to_value(value)?;

        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for entry in stack.iter_mut().rev() {
                if Rc::ptr_eq(&entry.id, &self.id) {
                    entry.data.insert(key, json_value);
                    entry.dirty = true;
                    return;
                }
            }
            panic!("EventContext missing on CONTEXT_STACK")
        });

        Ok(())
    }

    /// Returns a copy of the current context data.
    ///
    /// This method returns a snapshot of all key-value pairs stored in this context.
    /// The returned [`ContextData`] can be used to seed new contexts or passed to
    /// async tasks to maintain context across thread boundaries.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::EventContext;
    ///
    /// let mut ctx = EventContext::current();
    /// ctx.insert("request_id", &"abc123").unwrap();
    ///
    /// let data = ctx.data();
    /// // data now contains a copy of the context with request_id
    /// ```
    pub fn data(&self) -> ContextData {
        CONTEXT_STACK.with(|c| {
            let stack = c.borrow();
            for entry in stack.iter().rev() {
                if Rc::ptr_eq(&entry.id, &self.id) {
                    return entry.data.clone();
                }
            }
            panic!("EventContext missing on CONTEXT_STACK")
        })
    }

    /// Returns a snapshot of the context data only if it was mutated since
    /// the context was created (or since the last `data_if_dirty` call),
    /// clearing the dirty flag. Returns `None` when untouched, letting
    /// callers skip the clone entirely on the (overwhelmingly common)
    /// read-only path.
    pub(crate) fn data_if_dirty(&self) -> Option<ContextData> {
        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for entry in stack.iter_mut().rev() {
                if Rc::ptr_eq(&entry.id, &self.id) {
                    return if entry.dirty {
                        entry.dirty = false;
                        Some(entry.data.clone())
                    } else {
                        None
                    };
                }
            }
            None
        })
    }

    #[allow(unused_mut)]
    pub(crate) fn data_for_storing() -> ContextData {
        let mut data = Self::current().data();
        #[cfg(feature = "tracing-context")]
        {
            data = data.with_tracing_info();
        }
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack_depth() -> usize {
        CONTEXT_STACK.with(|c| c.borrow().len())
    }

    fn current_json() -> serde_json::Value {
        serde_json::to_value(EventContext::current().data()).unwrap()
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
        }

        let mut ctx = EventContext::current();
        assert_eq!(current_json(), serde_json::json!({}));
        let value = serde_json::json!({ "hello": "world" });
        ctx.insert("data", &value).unwrap();
        assert_eq!(current_json(), serde_json::json!({ "data": value }));
        insert_inner(&value);
        assert_eq!(
            current_json(),
            serde_json::json!({ "data": value, "new_data": value})
        );
        let new_value = serde_json::json!({ "hello": "new_world" });
        ctx.insert("data", &new_value).unwrap();
        assert_eq!(
            current_json(),
            serde_json::json!({ "data": new_value, "new_data": value})
        );
    }

    #[test]
    fn context_data_serializes_as_json_object_and_round_trips() {
        let mut ctx = EventContext::current();
        ctx.insert("request_id", &"req-123").unwrap();
        ctx.insert("nested", &serde_json::json!({ "a": 1 }))
            .unwrap();

        let data = ctx.data();
        assert_eq!(data.len(), 2);
        assert!(!data.is_empty());

        let json = serde_json::to_value(&data).unwrap();
        assert!(json.is_object());
        assert_eq!(
            json,
            serde_json::json!({ "request_id": "req-123", "nested": { "a": 1 } })
        );

        let round_tripped: ContextData = serde_json::from_value(json).unwrap();
        assert_eq!(
            round_tripped.lookup::<String>("request_id").unwrap(),
            Some("req-123".to_string())
        );
        assert_eq!(
            round_tripped.lookup::<serde_json::Value>("nested").unwrap(),
            Some(serde_json::json!({ "a": 1 }))
        );
        assert_eq!(round_tripped.lookup::<String>("missing").unwrap(), None);
    }

    #[test]
    fn context_data_insert_replaces_existing_key() {
        let mut ctx = EventContext::current();
        ctx.insert("key", &"first").unwrap();
        ctx.insert("key", &"second").unwrap();
        let data = ctx.data();
        assert_eq!(data.len(), 1);
        assert_eq!(
            data.lookup::<String>("key").unwrap(),
            Some("second".to_string())
        );
    }

    #[test]
    fn data_if_dirty_only_returns_data_after_mutation() {
        let mut ctx = EventContext::current();
        ctx.insert("data", &"value").unwrap();

        let seeded = EventContext::seed(ctx.data());
        assert!(seeded.data_if_dirty().is_none());

        let mut inner = EventContext::current();
        inner.insert("inner", &"mutation").unwrap();

        let dirty_data = seeded.data_if_dirty().expect("insert must mark dirty");
        assert_eq!(
            dirty_data.lookup::<String>("inner").unwrap(),
            Some("mutation".to_string())
        );
        // Flag is cleared by the read
        assert!(seeded.data_if_dirty().is_none());
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

    #[test]
    fn fork() {
        let mut ctx = EventContext::current();
        ctx.insert("original", &serde_json::json!("value")).unwrap();
        assert_eq!(stack_depth(), 1);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));

        let mut forked = EventContext::fork();
        assert_eq!(stack_depth(), 2);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));

        forked.insert("forked", &serde_json::json!("data")).unwrap();
        assert_eq!(
            current_json(),
            serde_json::json!({ "original": "value", "forked": "data" })
        );

        drop(forked);

        assert_eq!(stack_depth(), 1);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));
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
