use chrono::{DateTime, Utc};

use std::{sync::Arc, time::Duration};

use super::{
    artificial::ArtificialClock,
    config::ArtificialClockConfig,
    controller::ClockController,
    inner::ClockInner,
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
/// use es_entity::clock::{ClockHandle, ArtificialClockConfig};
///
/// // Real-time clock for production
/// let clock = ClockHandle::realtime();
///
/// // Artificial clock for testing - returns (handle, controller)
/// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig::manual());
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

    /// Create an artificial clock with the given configuration.
    ///
    /// Returns a tuple of `(ClockHandle, ClockController)`. The handle provides
    /// the common time interface, while the controller provides operations
    /// for advancing time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity::clock::{ClockHandle, ArtificialClockConfig, ArtificialMode};
    /// use chrono::Utc;
    ///
    /// // Manual mode - time only advances via controller.advance()
    /// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig::manual());
    ///
    /// // Auto mode - time advances 1000x faster than real time
    /// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig::auto(1000.0));
    ///
    /// // Start at a specific time
    /// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig {
    ///     start_at: Utc::now() - chrono::Duration::days(30),
    ///     mode: ArtificialMode::Manual,
    /// });
    /// ```
    pub fn artificial(config: ArtificialClockConfig) -> (Self, ClockController) {
        let clock = Arc::new(ArtificialClock::new(config));
        let handle = Self {
            inner: Arc::new(ClockInner::Artificial(Arc::clone(&clock))),
        };
        let controller = ClockController { clock };
        (handle, controller)
    }

    /// Get the current time.
    ///
    /// This is a fast, synchronous operation regardless of clock type.
    ///
    /// For real-time clocks, this returns `Utc::now()`.
    /// For artificial clocks, this returns the current artificial time.
    #[inline]
    pub fn now(&self) -> DateTime<Utc> {
        match &*self.inner {
            ClockInner::Realtime(rt) => rt.now(),
            ClockInner::Artificial(clock) => clock.now(),
        }
    }

    /// Sleep for the given duration.
    ///
    /// For real-time clocks, this delegates to `tokio::time::sleep`.
    /// For artificial clocks in manual mode, this waits until time is advanced.
    /// For artificial clocks in auto mode, this sleeps for a scaled real duration.
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

    /// Check if this clock is artificial (as opposed to realtime).
    pub fn is_artificial(&self) -> bool {
        matches!(&*self.inner, ClockInner::Artificial(_))
    }

    /// Get the current date (without time component).
    ///
    /// This is equivalent to `clock.now().date_naive()`.
    #[inline]
    pub fn today(&self) -> chrono::NaiveDate {
        self.now().date_naive()
    }

    /// Get the current artificial time, if this is an artificial clock that
    /// hasn't transitioned to realtime.
    ///
    /// Returns:
    /// - `None` for realtime clocks
    /// - `None` for artificial clocks that have transitioned to realtime
    /// - `Some(time)` for artificial clocks (manual or auto) that are still artificial
    ///
    /// This is useful for code that needs to cache time when running under
    /// artificial clocks but use fresh time for realtime clocks.
    pub fn artificial_now(&self) -> Option<DateTime<Utc>> {
        match &*self.inner {
            ClockInner::Realtime(_) => None,
            ClockInner::Artificial(clock) => {
                if clock.is_realtime() {
                    None
                } else {
                    Some(clock.now())
                }
            }
        }
    }
}

impl std::fmt::Debug for ClockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.inner {
            ClockInner::Realtime(_) => f.debug_struct("ClockHandle::Realtime").finish(),
            ClockInner::Artificial(clock) => f
                .debug_struct("ClockHandle::Artificial")
                .field("now", &clock.now())
                .finish(),
        }
    }
}
