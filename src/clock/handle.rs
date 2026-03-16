use chrono::{DateTime, Utc};

use std::{sync::Arc, time::Duration};

use super::{
    controller::ClockController,
    inner::ClockInner,
    manual::ManualClock,
    realtime::RealtimeClock,
    sleep::{ClockSleep, ClockTimeout},
};

pub use super::sleep::Elapsed;

/// A handle to a clock for getting time and performing time-based operations.
///
/// This is the main interface for time operations. It's cheap to clone and
/// can be shared across tasks and threads. All clones share the same underlying
/// clock, so they see consistent time.
///
/// # Creating a Clock
///
/// ```rust
/// use es_entity::clock::ClockHandle;
///
/// // Real-time clock for production
/// let clock = ClockHandle::realtime();
///
/// // Manual clock for testing - returns (handle, controller)
/// let (clock, ctrl) = ClockHandle::manual();
/// ```
///
/// # Basic Operations
///
/// ```rust
/// use es_entity::clock::ClockHandle;
/// use std::time::Duration;
///
/// # async fn example() {
/// let clock = ClockHandle::realtime();
///
/// // Get current time
/// let now = clock.now();
///
/// // Sleep for a duration
/// clock.sleep(Duration::from_secs(1)).await;
///
/// // Timeout a future
/// match clock.timeout(Duration::from_secs(5), some_async_operation()).await {
///     Ok(result) => println!("Completed: {:?}", result),
///     Err(_) => println!("Timed out"),
/// }
/// # }
/// # async fn some_async_operation() -> i32 { 42 }
/// ```
#[derive(Clone)]
pub struct ClockHandle {
    inner: Arc<ClockInner>,
}

impl ClockHandle {
    /// Create a real-time clock that uses the system clock and tokio timers.
    pub fn realtime() -> Self {
        Self {
            inner: Arc::new(ClockInner::Realtime(RealtimeClock)),
        }
    }

    /// Create a manual clock starting at the current time.
    ///
    /// Returns a tuple of `(ClockHandle, ClockController)`. The handle provides
    /// the common time interface, while the controller provides operations
    /// for advancing time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity::clock::ClockHandle;
    ///
    /// let (clock, ctrl) = ClockHandle::manual();
    /// ```
    pub fn manual() -> (Self, ClockController) {
        let clock = Arc::new(ManualClock::new());
        let handle = Self {
            inner: Arc::new(ClockInner::Manual(Arc::clone(&clock))),
        };
        let controller = ClockController { clock };
        (handle, controller)
    }

    /// Create a manual clock starting at a specific time.
    ///
    /// Returns a tuple of `(ClockHandle, ClockController)`. The handle provides
    /// the common time interface, while the controller provides operations
    /// for advancing time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity::clock::ClockHandle;
    /// use chrono::Utc;
    ///
    /// let (clock, ctrl) = ClockHandle::manual_at(Utc::now() - chrono::Duration::days(30));
    /// ```
    pub fn manual_at(start_at: DateTime<Utc>) -> (Self, ClockController) {
        let clock = Arc::new(ManualClock::new_at(start_at));
        let handle = Self {
            inner: Arc::new(ClockInner::Manual(Arc::clone(&clock))),
        };
        let controller = ClockController { clock };
        (handle, controller)
    }

    /// Get the current time.
    ///
    /// This is a fast, synchronous operation regardless of clock type.
    ///
    /// For real-time clocks, this returns `Utc::now()`.
    /// For manual clocks, this returns the current manual time.
    #[inline]
    pub fn now(&self) -> DateTime<Utc> {
        match &*self.inner {
            ClockInner::Realtime(rt) => rt.now(),
            ClockInner::Manual(clock) => clock.now(),
        }
    }

    /// Sleep for the given duration.
    ///
    /// For real-time clocks, this delegates to `tokio::time::sleep`.
    /// For manual clocks, this waits until time is advanced.
    pub fn sleep(&self, duration: Duration) -> ClockSleep {
        ClockSleep::new(&self.inner, duration)
    }

    /// Apply a timeout to a future.
    ///
    /// Returns `Ok(output)` if the future completes before the timeout,
    /// or `Err(Elapsed)` if the timeout expires first.
    pub fn timeout<F>(&self, duration: Duration, future: F) -> ClockTimeout<F>
    where
        F: std::future::Future,
    {
        ClockTimeout::new(&self.inner, duration, future)
    }

    /// Check if this clock is manual (as opposed to realtime).
    pub fn is_manual(&self) -> bool {
        matches!(&*self.inner, ClockInner::Manual(_))
    }

    /// Get the current date (without time component).
    ///
    /// This is equivalent to `clock.now().date_naive()`.
    #[inline]
    pub fn today(&self) -> chrono::NaiveDate {
        self.now().date_naive()
    }

    /// Get the current manual time, if this is a manual clock.
    ///
    /// Returns:
    /// - `None` for realtime clocks
    /// - `Some(time)` for manual clocks
    ///
    /// This is useful for code that needs to cache time when running under
    /// manual clocks but use fresh time for realtime clocks.
    pub fn manual_now(&self) -> Option<DateTime<Utc>> {
        match &*self.inner {
            ClockInner::Realtime(_) => None,
            ClockInner::Manual(clock) => Some(clock.now()),
        }
    }
}

impl std::fmt::Debug for ClockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.inner {
            ClockInner::Realtime(_) => f.debug_struct("ClockHandle::Realtime").finish(),
            ClockInner::Manual(clock) => f
                .debug_struct("ClockHandle::Manual")
                .field("now", &clock.now())
                .finish(),
        }
    }
}
