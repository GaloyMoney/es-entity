use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

use crate::config::SimulationConfig;
use crate::controller::ClockController;
use crate::inner::ClockInner;
use crate::realtime::RealtimeClock;
use crate::artificial::ArtificialClock;
use crate::sleep::{ClockSleep, ClockTimeout};

pub use crate::sleep::Elapsed;

/// A handle to a clock for getting time and performing time-based operations.
///
/// This is the main interface for time operations. It's cheap to clone and
/// can be shared across tasks and threads. All clones share the same underlying
/// clock, so they see consistent time.
///
/// # Creating a Clock
///
/// ```rust
/// use es_entity_time::{ClockHandle, SimulationConfig};
///
/// // Real-time clock for production
/// let clock = ClockHandle::realtime();
///
/// // Artificial clock for testing - returns (handle, controller)
/// let (clock, ctrl) = ClockHandle::artificial(SimulationConfig::manual());
/// ```
///
/// # Basic Operations
///
/// ```rust
/// use es_entity_time::ClockHandle;
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
    /// the common time interface, while the controller provides simulation-specific
    /// operations like advancing time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity_time::{ClockHandle, SimulationConfig, SimulationMode};
    /// use chrono::Utc;
    ///
    /// // Manual mode - time only advances via controller.advance()
    /// let (clock, ctrl) = ClockHandle::artificial(SimulationConfig::manual());
    ///
    /// // Auto mode - time advances 1000x faster than real time
    /// let (clock, ctrl) = ClockHandle::artificial(SimulationConfig::auto(1000.0));
    ///
    /// // Start at a specific time
    /// let (clock, ctrl) = ClockHandle::artificial(SimulationConfig {
    ///     start_at: Utc::now() - chrono::Duration::days(30),
    ///     mode: SimulationMode::Manual,
    /// });
    /// ```
    pub fn artificial(config: SimulationConfig) -> (Self, ClockController) {
        let sim = Arc::new(ArtificialClock::new(config));
        let handle = Self {
            inner: Arc::new(ClockInner::Artificial(Arc::clone(&sim))),
        };
        let controller = ClockController { sim };
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
            ClockInner::Artificial(sim) => sim.now(),
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
}

impl std::fmt::Debug for ClockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.inner {
            ClockInner::Realtime(_) => f.debug_struct("ClockHandle::Realtime").finish(),
            ClockInner::Artificial(sim) => f
                .debug_struct("ClockHandle::Artificial")
                .field("now", &sim.now())
                .finish(),
        }
    }
}
