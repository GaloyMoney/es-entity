use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

use crate::simulated::SimulatedClock;

/// Controller for simulated time operations.
///
/// This is only available for simulated clocks and provides methods to
/// advance time, set time, and inspect pending wake events.
///
/// Created alongside a [`ClockHandle`](crate::ClockHandle) via
/// [`ClockHandle::simulated()`](crate::ClockHandle::simulated).
#[derive(Clone)]
pub struct ClockController {
    pub(crate) sim: Arc<SimulatedClock>,
}

impl ClockController {
    /// Advance simulated time by the given duration.
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
    /// let (clock, ctrl) = ClockHandle::simulated(SimulationConfig::manual());
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
    /// ctrl.advance(Duration::from_secs(86400)).await;
    ///
    /// let wake_time = handle.await.unwrap();
    /// assert_eq!(wake_time, t0 + chrono::Duration::hours(1));
    /// # }
    /// ```
    pub async fn advance(&self, duration: Duration) -> usize {
        self.sim.advance(duration).await
    }

    /// Advance to the next pending wake event.
    ///
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
    /// let (clock, ctrl) = ClockHandle::simulated(SimulationConfig::manual());
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
    /// let t1 = ctrl.advance_to_next_wake().await;
    /// assert_eq!(t1, Some(t0 + chrono::Duration::seconds(60)));
    ///
    /// let t2 = ctrl.advance_to_next_wake().await;
    /// assert_eq!(t2, Some(t0 + chrono::Duration::seconds(120)));
    ///
    /// let t3 = ctrl.advance_to_next_wake().await;
    /// assert_eq!(t3, None); // No more pending wakes
    /// # }
    /// ```
    pub async fn advance_to_next_wake(&self) -> Option<DateTime<Utc>> {
        self.sim.advance_to_next_wake().await
    }

    /// Set the simulated time to a specific value.
    ///
    /// **Warning**: Unlike `advance()`, this does NOT process wake events in order.
    /// All tasks whose wake time has passed will see the new time when they wake.
    /// Use this for "jump ahead, don't care about intermediate events" scenarios.
    ///
    /// For deterministic testing, prefer `advance()` or `advance_to_next_wake()`.
    pub fn set_time(&self, time: DateTime<Utc>) {
        self.sim.set_time(time);
        // Wake all tasks that are now past their wake time
        self.sim.wake_tasks_at(time.timestamp_millis());
    }

    /// Get the number of pending wake events.
    ///
    /// This is useful for testing to verify that tasks have registered
    /// their sleeps before advancing time.
    pub fn pending_wake_count(&self) -> usize {
        self.sim.pending_wake_count()
    }

    /// Get the current simulated time.
    ///
    /// This is equivalent to calling `now()` on the associated `ClockHandle`.
    pub fn now(&self) -> DateTime<Utc> {
        self.sim.now()
    }
}

impl std::fmt::Debug for ClockController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockController")
            .field("now", &self.sim.now())
            .field("pending_wakes", &self.sim.pending_wake_count())
            .finish()
    }
}
