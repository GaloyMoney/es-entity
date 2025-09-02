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

#[cfg(feature = "tracing")]
mod tracing;
mod with_event_context;

use serde::Serialize;

use std::{borrow::Cow, cell::RefCell, rc::Rc};

pub use with_event_context::*;

/// Immutable context data that can be safely shared across thread boundaries.
///
/// This struct holds key-value pairs of context information that gets attached
/// to events when they are persisted. It uses an immutable HashMap internally
/// for efficient cloning and thread-safe sharing of data snapshots.
///
/// `ContextData` is `Send` and can be passed between threads, unlike [`EventContext`]
/// which is thread-local. This makes it suitable for transferring context across
/// async boundaries via the [`WithEventContext`] trait.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct ContextData(im::HashMap<Cow<'static, str>, serde_json::Value>);

impl ContextData {
    fn new() -> Self {
        Self(im::HashMap::new())
    }

    fn insert(&mut self, key: &'static str, value: serde_json::Value) {
        self.0 = self.0.update(Cow::Borrowed(key), value);
    }
}

struct StackEntry {
    id: Rc<()>,
    data: ContextData,
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

    /// Serializes the current context data to JSON.
    ///
    /// This method is primarily used internally by the event persistence system
    /// to store context data alongside events in the database. It converts all
    /// context key-value pairs into a single JSON object.
    ///
    /// # Returns
    ///
    /// Returns a `serde_json::Value` containing all context data, or an error
    /// if serialization fails.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use es_entity::context::EventContext;
    ///
    /// let mut ctx = EventContext::current();
    /// ctx.insert("user_id", &"12345").unwrap();
    ///
    /// let json = ctx.as_json().unwrap();
    /// // json is now: {"user_id": "12345"}
    /// ```
    #[allow(unused_mut)]
    pub fn as_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        let mut data = self.data();
        #[cfg(feature = "tracing")]
        {
            // Only inject if not already present
            if !data.0.contains_key(&Cow::Borrowed("tracing")) {
                let tracing = tracing::extract_current_tracing_context();
                data.insert("tracing", serde_json::to_value(&tracing)?);
            }
        }
        serde_json::to_value(&data)
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
