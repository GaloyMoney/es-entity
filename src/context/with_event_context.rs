use pin_project::pin_project;

use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::{ContextData, EventContext};

/// Extension trait for propagating event context across async boundaries.
///
/// This trait is automatically implemented for all `Future` types and provides
/// the `with_event_context` method to carry context data across async operations
/// like `tokio::spawn`, `tokio::task::yield_now()`, and other async boundaries
/// where thread-local storage is not preserved.
///
/// # Examples
///
/// ```rust
/// use es_entity::context::{EventContext, WithEventContext};
///
/// async fn example() {
///     let mut ctx = EventContext::current();
///     ctx.insert("request_id", &"abc123").unwrap();
///
///     let data = ctx.data();
///     tokio::spawn(async {
///         // Context is available here
///         let current = EventContext::current();
///         // current now has the request_id from the parent
///     }.with_event_context(data)).await.unwrap();
/// }
/// ```
pub trait WithEventContext: Future {
    /// Wraps this future with event context data.
    ///
    /// This method ensures that when the future is polled, the provided context
    /// data will be available as the current event context. This is essential
    /// for maintaining context across async boundaries where the original
    /// thread-local context is not available.
    ///
    /// # Arguments
    ///
    /// * `context_data` - The context data to make available during future execution
    ///
    /// # Returns
    ///
    /// Returns an [`EventContextFuture`] that will poll the wrapped future with
    /// the provided context active.
    fn with_event_context(self, context_data: ContextData) -> EventContextFuture<Self>
    where
        Self: Sized,
    {
        EventContextFuture {
            future: self,
            context_data: Some(context_data),
        }
    }
}

impl<F: Future> WithEventContext for F {}

/// A future wrapper that provides event context during polling.
///
/// This struct is created by the `with_event_context` method and should not
/// be constructed directly. It ensures that the wrapped future has access
/// to the specified event context data whenever it is polled.
///
/// The future maintains context isolation - the context is only active
/// during the polling of the wrapped future and does not leak to other
/// concurrent operations.
///
/// Seeding is lazy: on each poll the context data is parked on a
/// thread-local pending-seed stack, and a real [`EventContext`] entry is
/// only materialized if the inner future actually observes the context
/// (via [`EventContext::current`] or [`EventContext::seed`]) during that
/// poll. Polls that never touch the context — the overwhelming majority —
/// pay neither an allocation nor a clone.
///
/// [`EventContext`]: super::EventContext
/// [`EventContext::current`]: super::EventContext::current
/// [`EventContext::seed`]: super::EventContext::seed
#[pin_project]
pub struct EventContextFuture<F> {
    #[pin]
    future: F,
    /// `Some` between polls; taken for the duration of each poll while the
    /// data is parked on the pending-seed stack.
    context_data: Option<ContextData>,
}

impl<F: Future> Future for EventContextFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let data = this
            .context_data
            .take()
            .expect("EventContextFuture must not be polled after a panic");
        let guard = SeedGuard::enter(data);
        let res = this.future.poll(cx);
        *this.context_data = Some(guard.finish());
        res
    }
}

/// A context seed deferred by [`EventContextFuture`]: the wrapper's
/// [`ContextData`] is parked here on poll entry instead of eagerly becoming
/// a context-stack entry. A real entry is only materialized — see
/// [`materialize_pending_seeds`] — if the inner future actually observes the
/// context during that poll. Most polls of a request/job future never do, so
/// the common case skips the entry allocation, the push/pop, and the clone
/// entirely.
struct PendingSeed {
    /// `Some` until materialized; then moved into the stack entry.
    data: Option<ContextData>,
    /// Handle to the materialized entry, if any. Dropping it removes the
    /// entry via the normal [`EventContext`] drop logic.
    ctx: Option<EventContext>,
}

thread_local! {
    static PENDING_SEEDS: RefCell<Vec<PendingSeed>> = const { RefCell::new(Vec::new()) };
}

/// Materializes every pending seed into a real context-stack entry, in push
/// order.
///
/// [`EventContext`] calls this before any operation that observes or pushes
/// onto the stack ([`EventContext::current`] / [`EventContext::seed`]). This
/// preserves the invariant that pending seeds always sit logically *above*
/// every stack entry that existed when they were parked: any new entry is
/// pushed after the seeds have been materialized beneath it.
pub(super) fn materialize_pending_seeds() {
    PENDING_SEEDS.with(|p| {
        let mut pending = p.borrow_mut();
        for seed in pending.iter_mut() {
            if seed.ctx.is_none() {
                let data = seed.data.take().expect("unmaterialized seed retains data");
                seed.ctx = Some(EventContext::push_entry(data));
            }
        }
    })
}

/// RAII token for one [`EventContextFuture`] poll invocation.
///
/// [`enter`](Self::enter) parks the wrapper's data as a pending seed;
/// [`finish`](Self::finish) (normal exit) pops it and returns the data to
/// store back into the wrapper — either untouched (fast path: the poll never
/// observed the context) or harvested from the materialized entry. If the
/// inner poll panics, `Drop` pops the record and discards any materialized
/// handle so the thread-local stacks stay balanced.
#[must_use]
struct SeedGuard(());

impl SeedGuard {
    fn enter(data: ContextData) -> Self {
        PENDING_SEEDS.with(|p| {
            p.borrow_mut().push(PendingSeed {
                data: Some(data),
                ctx: None,
            })
        });
        SeedGuard(())
    }

    fn finish(self) -> ContextData {
        let seed = PENDING_SEEDS
            .with(|p| p.borrow_mut().pop())
            .expect("SeedGuard::finish: pending-seed stack is empty");
        std::mem::forget(self);
        match seed {
            // Materialized: harvest the (possibly mutated) data. Dropping the
            // handle removes the entry unless the inner future kept its own
            // handle across the poll — same lifecycle as an eager seed.
            PendingSeed { ctx: Some(ctx), .. } => ctx.data(),
            // Fast path: the poll never observed the context; hand the data
            // back unchanged without ever having touched the context stack.
            PendingSeed { data, .. } => data.expect("unmaterialized seed retains data"),
        }
    }
}

impl Drop for SeedGuard {
    fn drop(&mut self) {
        // Unwind path: the wrapped poll panicked. Pop the record; dropping a
        // materialized handle removes its stack entry via EventContext::drop.
        // Write-back is skipped, matching the eager-seeding behavior.
        let _ = PENDING_SEEDS.with(|p| p.borrow_mut().pop());
    }
}

/// Test-only visibility into the pending-seed stack for the context tests.
#[cfg(test)]
pub(super) fn pending_seed_depth() -> usize {
    PENDING_SEEDS.with(|p| p.borrow().len())
}
