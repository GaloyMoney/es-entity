use pin_project::{pin_project, pinned_drop};
use tokio::time::Sleep;

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use super::{
    artificial::{ArtificialClock, next_sleep_id},
    inner::ClockInner,
};

/// A future that completes after a duration has elapsed on the clock.
///
/// Created by [`ClockHandle::sleep`](crate::ClockHandle::sleep).
#[pin_project(PinnedDrop)]
pub struct ClockSleep {
    #[pin]
    inner: ClockSleepInner,
}

#[pin_project(project = ClockSleepInnerProj)]
enum ClockSleepInner {
    Realtime {
        #[pin]
        sleep: Sleep,
    },
    ArtificialAuto {
        #[pin]
        sleep: Sleep,
        wake_at_ms: i64,
        clock: Arc<ArtificialClock>,
    },
    ArtificialManual {
        wake_at_ms: i64,
        sleep_id: u64,
        clock: Arc<ArtificialClock>,
        registered: bool,
    },
}

impl ClockSleep {
    pub(crate) fn new(clock_inner: &ClockInner, duration: Duration) -> Self {
        let inner = match clock_inner {
            ClockInner::Realtime(rt) => ClockSleepInner::Realtime {
                sleep: rt.sleep(duration),
            },
            ClockInner::Artificial(sim) => {
                let wake_at_ms = sim.now_ms() + duration.as_millis() as i64;

                if sim.is_manual() {
                    ClockSleepInner::ArtificialManual {
                        wake_at_ms,
                        sleep_id: next_sleep_id(),
                        clock: Arc::clone(sim),
                        registered: false,
                    }
                } else {
                    // Auto-advance mode uses real tokio sleep with scaled duration
                    let real_duration = sim.real_duration(duration);
                    ClockSleepInner::ArtificialAuto {
                        sleep: tokio::time::sleep(real_duration),
                        wake_at_ms,
                        clock: Arc::clone(sim),
                    }
                }
            }
        };

        Self { inner }
    }
}

impl Future for ClockSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.project();

        match this.inner.project() {
            ClockSleepInnerProj::Realtime { sleep } => sleep.poll(cx),

            ClockSleepInnerProj::ArtificialAuto {
                sleep,
                wake_at_ms,
                clock,
            } => {
                // Check if artificial time has reached wake time
                if clock.now_ms() >= *wake_at_ms {
                    return Poll::Ready(());
                }
                // Otherwise wait for real timer
                sleep.poll(cx)
            }

            ClockSleepInnerProj::ArtificialManual {
                wake_at_ms,
                sleep_id,
                clock,
                registered,
            } => {
                // Check if we've reached wake time
                if clock.now_ms() >= *wake_at_ms {
                    return Poll::Ready(());
                }

                // Register for wake notification if not already done
                if !*registered {
                    clock.register_wake(*wake_at_ms, *sleep_id, cx.waker().clone());
                    *registered = true;
                }

                Poll::Pending
            }
        }
    }
}

#[pinned_drop]
impl PinnedDrop for ClockSleep {
    fn drop(self: Pin<&mut Self>) {
        // Clean up pending wake registration if cancelled
        if let ClockSleepInner::ArtificialManual {
            sleep_id,
            clock,
            registered: true,
            ..
        } = &self.inner
        {
            clock.cancel_wake(*sleep_id);
        }
    }
}

/// A future that completes with a timeout after a duration has elapsed on the clock.
///
/// Created by [`ClockHandle::timeout`](crate::ClockHandle::timeout).
#[pin_project]
pub struct ClockTimeout<F> {
    #[pin]
    future: F,
    #[pin]
    sleep: ClockSleep,
    completed: bool,
}

impl<F> ClockTimeout<F> {
    pub(crate) fn new(clock_inner: &ClockInner, duration: Duration, future: F) -> Self {
        Self {
            future,
            sleep: ClockSleep::new(clock_inner, duration),
            completed: false,
        }
    }
}

impl<F: Future> Future for ClockTimeout<F> {
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if *this.completed {
            panic!("ClockTimeout polled after completion");
        }

        // Check the future first
        if let Poll::Ready(output) = this.future.poll(cx) {
            *this.completed = true;
            return Poll::Ready(Ok(output));
        }

        // Check if timeout elapsed
        if let Poll::Ready(()) = this.sleep.poll(cx) {
            *this.completed = true;
            return Poll::Ready(Err(Elapsed));
        }

        Poll::Pending
    }
}

/// Error returned when a timeout expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}
