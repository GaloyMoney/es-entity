use pin_project::pin_project;

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::{ContextData, EventContext};

pub trait WithEventContext: Future {
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
