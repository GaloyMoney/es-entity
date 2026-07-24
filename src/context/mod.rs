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
//!
//! # Transient Entries / PII
//!
//! Entries added via [`EventContext::insert_transient`] participate in all of the
//! propagation above (forks, threads, async boundaries) but are excluded whenever
//! `ContextData` is serialized — in particular they are never written to the
//! `context` column. Use transient entries for request-scoped metadata that
//! in-process consumers need but that must not be persisted into immutable event
//! streams, such as client IPs, user agents, or other personal data. Consumers
//! may still store such data explicitly in mutable tables that can honor
//! erasure requests; transient entries just guarantee the event-context
//! machinery never writes it anywhere on its own:
//!
//! ```rust
//! use es_entity::context::EventContext;
//!
//! let mut ctx = EventContext::current();
//! ctx.insert("request_id", &"req-123").unwrap();          // persisted with events
//! ctx.insert_transient("client_ip", &"203.0.113.7").unwrap(); // in-process only
//!
//! // Both are visible via lookup...
//! let ip: Option<String> = ctx.data().lookup("client_ip").unwrap();
//! assert_eq!(ip.as_deref(), Some("203.0.113.7"));
//!
//! // ...but only persisted entries survive serialization.
//! let json = serde_json::to_value(ctx.data()).unwrap();
//! assert!(json.get("request_id").is_some());
//! assert!(json.get("client_ip").is_none());
//! ```

mod sqlx;
mod tracing;
mod with_event_context;

use serde::{Deserialize, Serialize};

use std::{borrow::Cow, cell::RefCell, rc::Rc};

pub use tracing::*;
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
///
/// # Persisted vs transient entries
///
/// Entries come in two flavors:
///
/// - **Persisted** entries (added via [`EventContext::insert`]) are serialized
///   into the `context` column when events are stored.
/// - **Transient** entries (added via [`EventContext::insert_transient`]) behave
///   identically in-process — they propagate across forks, threads, and async
///   boundaries — but are excluded from serialization, so they never leave the
///   process. Use them for data that must not end up in immutable event streams
///   (e.g. request IP / user agent and other personal data).
///
/// The exclusion is enforced by the `Serialize` implementation itself: any code
/// path that serializes `ContextData` (the repository persist path, sqlx
/// encoding, `serde_json::to_value`, ...) only ever sees persisted entries.
#[derive(Debug, Clone)]
pub struct ContextData {
    persisted: im::HashMap<Cow<'static, str>, serde_json::Value>,
    transient: im::HashMap<Cow<'static, str>, serde_json::Value>,
}

/// The wire format is the plain JSON map of the persisted entries — identical
/// to the format written before transient entries existed. Transient entries
/// are deliberately excluded.
impl Serialize for ContextData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.persisted.serialize(serializer)
    }
}

/// Deserializes the plain JSON map into persisted entries; transient entries
/// start out empty (they never survive serialization boundaries).
impl<'de> Deserialize<'de> for ContextData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let persisted = im::HashMap::deserialize(deserializer)?;
        Ok(Self {
            persisted,
            transient: im::HashMap::new(),
        })
    }
}

impl ContextData {
    fn new() -> Self {
        Self {
            persisted: im::HashMap::new(),
            transient: im::HashMap::new(),
        }
    }

    fn insert(&mut self, key: &'static str, value: serde_json::Value) {
        self.persisted = self.persisted.update(Cow::Borrowed(key), value);
    }

    fn insert_transient(&mut self, key: &'static str, value: serde_json::Value) {
        self.transient = self.transient.update(Cow::Borrowed(key), value);
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

    /// Looks up a value by key, checking transient entries first, then
    /// persisted ones (a transient entry shadows a persisted entry with the
    /// same key). Callers do not need to know which insert produced the key.
    pub fn lookup<T: serde::de::DeserializeOwned>(
        &self,
        key: &'static str,
    ) -> Result<Option<T>, serde_json::Error> {
        let Some(val) = self.transient.get(key).or_else(|| self.persisted.get(key)) else {
            return Ok(None);
        };
        serde_json::from_value(val.clone()).map(Some)
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

    /// Inserts a transient key-value pair into the current context.
    ///
    /// Transient entries behave exactly like entries added via
    /// [`insert`](Self::insert) while in-process: they are visible through
    /// [`ContextData::lookup`], are inherited by [`fork`](Self::fork)ed
    /// contexts, and propagate across threads and async boundaries via
    /// [`seed`](Self::seed) / [`WithEventContext`]. However, they are **never
    /// serialized** — when events are persisted, transient entries are
    /// excluded from the `context` column (and from any other serialization
    /// of [`ContextData`]).
    ///
    /// Use this for request-scoped metadata that must not be written into
    /// immutable event streams, such as client IP addresses or user agents.
    /// In-process consumers can still read the entry via
    /// [`ContextData::lookup`] and persist it deliberately in a mutable store
    /// that can honor erasure requests (e.g. an audit table) — transient only
    /// means the event-context machinery itself never serializes it.
    ///
    /// On key collision a transient entry shadows a persisted entry in
    /// [`ContextData::lookup`].
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
    /// ctx.insert_transient("client_ip", &"203.0.113.7").unwrap();
    ///
    /// // Visible in-process...
    /// let ip: Option<String> = ctx.data().lookup("client_ip").unwrap();
    /// assert_eq!(ip.as_deref(), Some("203.0.113.7"));
    ///
    /// // ...but excluded from serialization.
    /// let json = serde_json::to_value(ctx.data()).unwrap();
    /// assert!(json.get("client_ip").is_none());
    /// ```
    pub fn insert_transient<T: Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), serde_json::Error> {
        let json_value = serde_json::to_value(value)?;

        CONTEXT_STACK.with(|c| {
            let mut stack = c.borrow_mut();
            for entry in stack.iter_mut().rev() {
                if Rc::ptr_eq(&entry.id, &self.id) {
                    entry.data.insert_transient(key, json_value);
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

    fn current_lookup(key: &'static str) -> Option<serde_json::Value> {
        EventContext::current().data().lookup(key).unwrap()
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
    fn insert_transient() {
        let mut ctx = EventContext::current();
        ctx.insert("persisted", &serde_json::json!("value"))
            .unwrap();
        ctx.insert_transient("transient", &serde_json::json!("secret"))
            .unwrap();

        // Both entries are visible via lookup
        assert_eq!(
            current_lookup("persisted"),
            Some(serde_json::json!("value"))
        );
        assert_eq!(
            current_lookup("transient"),
            Some(serde_json::json!("secret"))
        );

        // Serialization only contains persisted entries
        assert_eq!(current_json(), serde_json::json!({ "persisted": "value" }));
    }

    #[test]
    fn transient_shadows_persisted_on_lookup() {
        let mut ctx = EventContext::current();
        ctx.insert("key", &serde_json::json!("persisted")).unwrap();
        ctx.insert_transient("key", &serde_json::json!("transient"))
            .unwrap();

        assert_eq!(current_lookup("key"), Some(serde_json::json!("transient")));

        // The wire format still carries the persisted value
        assert_eq!(current_json(), serde_json::json!({ "key": "persisted" }));
    }

    #[test]
    fn legacy_wire_format_roundtrip() {
        let json = serde_json::json!({ "audit_info": { "sub": "user-1" } });
        let data: ContextData = serde_json::from_value(json.clone()).unwrap();

        // Plain-map JSON deserializes into persisted entries
        assert_eq!(
            data.lookup::<serde_json::Value>("audit_info").unwrap(),
            Some(serde_json::json!({ "sub": "user-1" }))
        );

        // Re-serializing produces the identical plain map
        assert_eq!(serde_json::to_value(&data).unwrap(), json);
    }

    #[test]
    fn thread_isolation() {
        let mut ctx = EventContext::current();
        let value = serde_json::json!({ "main": "thread" });
        ctx.insert("data", &value).unwrap();
        ctx.insert_transient("secret", &serde_json::json!("pii"))
            .unwrap();
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
            // Transient entry crossed the thread boundary but stays out of serialization
            assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
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
            assert_eq!(
                current_lookup("async_secret"),
                Some(serde_json::json!("pii"))
            );
        }

        let mut ctx = EventContext::current();
        assert_eq!(current_json(), serde_json::json!({}));

        let value = serde_json::json!({ "test": "async" });
        ctx.insert("async_data", &value).unwrap();
        ctx.insert_transient("async_secret", &serde_json::json!("pii"))
            .unwrap();
        assert_eq!(current_json(), serde_json::json!({ "async_data": value }));

        inner_async().await;

        assert_eq!(
            current_json(),
            serde_json::json!({ "async_data": value, "async_inner": "value" })
        );
        assert_eq!(
            current_lookup("async_secret"),
            Some(serde_json::json!("pii"))
        );
    }

    #[test]
    fn fork() {
        let mut ctx = EventContext::current();
        ctx.insert("original", &serde_json::json!("value")).unwrap();
        ctx.insert_transient("secret", &serde_json::json!("pii"))
            .unwrap();
        assert_eq!(stack_depth(), 1);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));

        let mut forked = EventContext::fork();
        assert_eq!(stack_depth(), 2);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));
        // Transient entries are inherited by forked contexts
        assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));

        forked.insert("forked", &serde_json::json!("data")).unwrap();
        forked
            .insert_transient("forked_secret", &serde_json::json!("pii2"))
            .unwrap();
        assert_eq!(
            current_json(),
            serde_json::json!({ "original": "value", "forked": "data" })
        );
        assert_eq!(
            current_lookup("forked_secret"),
            Some(serde_json::json!("pii2"))
        );

        drop(forked);

        assert_eq!(stack_depth(), 1);
        assert_eq!(current_json(), serde_json::json!({ "original": "value" }));
        // Transient entries respect the same fork isolation as persisted ones
        assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
        assert_eq!(current_lookup("forked_secret"), None);
    }

    #[tokio::test]
    async fn with_event_context_spawned() {
        let mut ctx = EventContext::current();
        ctx.insert("parent", &serde_json::json!("context")).unwrap();
        ctx.insert_transient("secret", &serde_json::json!("pii"))
            .unwrap();

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
                // Transient entry propagated into the spawned task
                assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
                tokio::task::yield_now().await;
                assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
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
        ctx.insert_transient("secret", &serde_json::json!("pii"))
            .unwrap();

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
                // Transient entry survived tokio::spawn onto another thread
                assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
                let data = EventContext::current().data();
                tokio::task::yield_now().with_event_context(data).await;
                assert_eq!(current_lookup("secret"), Some(serde_json::json!("pii")));
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
