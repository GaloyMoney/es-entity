use pin_project::pin_project;

use std::{
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
            context_data,
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
#[pin_project]
pub struct EventContextFuture<F> {
    #[pin]
    future: F,
    context_data: ContextData,
}

impl<F: Future> Future for EventContextFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let ctx = EventContext::seed(this.context_data.clone());
        let res = this.future.poll(cx);
        *this.context_data = ctx.data();
        res
    }
}
