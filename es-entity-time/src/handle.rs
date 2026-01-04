use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

use crate::config::SimulationConfig;
use crate::inner::ClockInner;
use crate::realtime::RealtimeClock;
use crate::simulated::SimulatedClock;
pub use crate::sleep::Elapsed;
use crate::sleep::{ClockSleep, ClockTimeout};

/// A handle to a clock for getting time and performing time-based operations.
///
/// This is the main interface for time operations. It's cheap to clone and
/// can be shared across tasks and threads. All clones share the same underlying
/// clock, so they see consistent time.
///
/// # Creating a Clock
///
/// ```rust
/// use es_entity_time::{ClockHandle, SimulationConfig, SimulationMode};
///
/// // Real-time clock for production
/// let clock = ClockHandle::realtime();
///
/// // Simulated clock for testing (manual mode)
/// let clock = ClockHandle::simulated(SimulationConfig::manual());
///
/// // Simulated clock with auto-advance (1000x faster)
/// let clock = ClockHandle::simulated(SimulationConfig::auto(1000.0));
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

    /// Create a simulated clock with the given configuration.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity_time::{ClockHandle, SimulationConfig, SimulationMode};
    /// use chrono::Utc;
    ///
    /// // Manual mode - time only advances via advance()
    /// let clock = ClockHandle::simulated(SimulationConfig::manual());
    ///
    /// // Auto mode - time advances 1000x faster than real time
    /// let clock = ClockHandle::simulated(SimulationConfig::auto(1000.0));
    ///
    /// // Start at a specific time
    /// let clock = ClockHandle::simulated(SimulationConfig {
    ///     start_at: Utc::now() - chrono::Duration::days(30),
    ///     mode: SimulationMode::Manual,
    /// });
    /// ```
    pub fn simulated(config: SimulationConfig) -> Self {
        Self {
            inner: Arc::new(ClockInner::Simulated(Arc::new(SimulatedClock::new(config)))),
        }
    }

    /// Get the current time.
    ///
    /// This is a fast, synchronous operation regardless of clock type.
    ///
    /// For real-time clocks, this returns `Utc::now()`.
    /// For simulated clocks, this returns the current simulated time.
    #[inline]
    pub fn now(&self) -> DateTime<Utc> {
        match &*self.inner {
            ClockInner::Realtime(rt) => rt.now(),
            ClockInner::Simulated(sim) => sim.now(),
        }
    }

    /// Sleep for the given duration.
    ///
    /// For real-time clocks, this delegates to `tokio::time::sleep`.
    /// For simulated clocks in manual mode, this waits until time is advanced.
    /// For simulated clocks in auto mode, this sleeps for a scaled real duration.
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

    /// Advance simulated time by the given duration.
    ///
    /// This only works for simulated clocks in manual mode. For other clock
    /// types, this is a no-op that returns 0.
    ///
    /// Wake events are processed in chronological order. If you advance by
    /// 1 day and there are sleeps scheduled at 1 hour and 2 hours, they will
    /// wake at their scheduled times (seeing the correct `now()` value),
    /// not at +1 day.
    ///
    /// Returns the number of wake events that were processed.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity_time::{ClockHandle, SimulationConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let clock = ClockHandle::simulated(SimulationConfig::manual());
    /// let t0 = clock.now();
    ///
    /// let clock2 = clock.clone();
    /// let handle = tokio::spawn(async move {
    ///     clock2.sleep(Duration::from_secs(3600)).await;
    ///     clock2.now() // Will be t0 + 1 hour
    /// });
    ///
    /// tokio::task::yield_now().await; // Let task register its sleep
    ///
    /// // Advance 1 day - task wakes at exactly +1 hour
    /// clock.advance(Duration::from_secs(86400)).await;
    ///
    /// let wake_time = handle.await.unwrap();
    /// assert_eq!(wake_time, t0 + chrono::Duration::hours(1));
    /// # }
    /// ```
    pub async fn advance(&self, duration: Duration) -> usize {
        match &*self.inner {
            ClockInner::Realtime(_) => 0,
            ClockInner::Simulated(sim) => sim.advance(duration).await,
        }
    }

    /// Advance to the next pending wake event.
    ///
    /// This only works for simulated clocks in manual mode.
    /// Returns the time that was advanced to, or `None` if there are no
    /// pending wake events.
    ///
    /// This is useful for step-by-step testing where you want to process
    /// events one at a time.
    ///
    /// # Example
    ///
    /// ```rust
    /// use es_entity_time::{ClockHandle, SimulationConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let clock = ClockHandle::simulated(SimulationConfig::manual());
    /// let t0 = clock.now();
    ///
    /// // Spawn tasks with different sleep durations
    /// let c = clock.clone();
    /// tokio::spawn(async move { c.sleep(Duration::from_secs(60)).await; });
    /// let c = clock.clone();
    /// tokio::spawn(async move { c.sleep(Duration::from_secs(120)).await; });
    ///
    /// tokio::task::yield_now().await;
    ///
    /// // Step through one wake at a time
    /// let t1 = clock.advance_to_next_wake().await;
    /// assert_eq!(t1, Some(t0 + chrono::Duration::seconds(60)));
    ///
    /// let t2 = clock.advance_to_next_wake().await;
    /// assert_eq!(t2, Some(t0 + chrono::Duration::seconds(120)));
    ///
    /// let t3 = clock.advance_to_next_wake().await;
    /// assert_eq!(t3, None); // No more pending wakes
    /// # }
    /// ```
    pub async fn advance_to_next_wake(&self) -> Option<DateTime<Utc>> {
        match &*self.inner {
            ClockInner::Realtime(_) => None,
            ClockInner::Simulated(sim) => sim.advance_to_next_wake().await,
        }
    }

    /// Set the simulated time to a specific value.
    ///
    /// This only works for simulated clocks. For real-time clocks, this is a no-op.
    ///
    /// **Warning**: Unlike `advance()`, this does NOT process wake events in order.
    /// All tasks whose wake time has passed will see the new time when they wake.
    /// Use this for "jump ahead, don't care about intermediate events" scenarios.
    ///
    /// For deterministic testing, prefer `advance()` or `advance_to_next_wake()`.
    pub fn set_time(&self, time: DateTime<Utc>) {
        if let ClockInner::Simulated(sim) = &*self.inner {
            sim.set_time(time);
            // Wake all tasks that are now past their wake time
            sim.wake_tasks_at(time.timestamp_millis());
        }
    }

    /// Get the number of pending wake events.
    ///
    /// This is mainly useful for testing to verify that tasks have registered
    /// their sleeps before advancing time.
    pub fn pending_wake_count(&self) -> usize {
        match &*self.inner {
            ClockInner::Realtime(_) => 0,
            ClockInner::Simulated(sim) => sim.pending_wake_count(),
        }
    }

    /// Check if this is a simulated clock.
    pub fn is_simulated(&self) -> bool {
        matches!(&*self.inner, ClockInner::Simulated(_))
    }

    /// Check if this is a real-time clock.
    pub fn is_realtime(&self) -> bool {
        matches!(&*self.inner, ClockInner::Realtime(_))
    }
}

impl std::fmt::Debug for ClockHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &*self.inner {
            ClockInner::Realtime(_) => f.debug_struct("ClockHandle::Realtime").finish(),
            ClockInner::Simulated(sim) => f
                .debug_struct("ClockHandle::Simulated")
                .field("now", &sim.now())
                .field("pending_wakes", &sim.pending_wake_count())
                .finish(),
        }
    }
}
