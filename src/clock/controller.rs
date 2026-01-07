use chrono::{DateTime, Utc};

use std::{sync::Arc, time::Duration};

use super::artificial::ArtificialClock;

/// Controller for artificial time operations.
///
/// This is only available for artificial clocks and provides methods to
/// advance time, set time, and inspect pending wake events.
///
/// Created alongside a [`ClockHandle`](crate::ClockHandle) via
/// [`ClockHandle::artificial()`](crate::ClockHandle::artificial).
#[derive(Clone)]
pub struct ClockController {
    pub(crate) clock: Arc<ArtificialClock>,
}

impl ClockController {
    /// Advance artificial time by the given duration.
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
    /// use es_entity::clock::{ClockHandle, ArtificialClockConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig::manual());
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
        self.clock.advance(duration).await
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
    /// use es_entity::clock::{ClockHandle, ArtificialClockConfig};
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let (clock, ctrl) = ClockHandle::artificial(ArtificialClockConfig::manual());
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
        self.clock.advance_to_next_wake().await
    }

    /// Set the artificial time to a specific value.
    ///
    /// **Warning**: Unlike `advance()`, this does NOT process wake events in order.
    /// All tasks whose wake time has passed will see the new time when they wake.
    /// Use this for "jump ahead, don't care about intermediate events" scenarios.
    ///
    /// For deterministic testing, prefer `advance()` or `advance_to_next_wake()`.
    pub fn set_time(&self, time: DateTime<Utc>) {
        self.clock.set_time(time);
        // Wake all tasks that are now past their wake time
        self.clock.wake_tasks_at(time.timestamp_millis());
    }

    /// Get the number of pending wake events.
    ///
    /// This is useful for testing to verify that tasks have registered
    /// their sleeps before advancing time.
    pub fn pending_wake_count(&self) -> usize {
        self.clock.pending_wake_count()
    }

    /// Get the current artificial time.
    ///
    /// This is equivalent to calling `now()` on the associated `ClockHandle`.
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Transition to realtime mode.
    ///
    /// After this call:
    /// - `now()` returns `Utc::now()`
    /// - `sleep()` uses real tokio timers
    /// - `advance()` becomes a no-op
    ///
    /// Pending sleeps are woken immediately and will re-register using real timers.
    pub fn transition_to_realtime(&self) {
        self.clock.transition_to_realtime();
    }

    /// Check if clock has transitioned to realtime.
    pub fn is_realtime(&self) -> bool {
        self.clock.is_realtime()
    }

    /// Clear all pending wake events.
    pub fn clear_pending_wakes(&self) {
        self.clock.clear_pending_wakes();
    }

    /// Reset clock to a specific time and clear all pending wakes.
    ///
    /// Useful for test isolation between test cases.
    pub fn reset_to(&self, time: DateTime<Utc>) {
        self.clock.set_time(time);
        self.clock.clear_pending_wakes();
    }
}

impl std::fmt::Debug for ClockController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClockController")
            .field("now", &self.clock.now())
            .field("pending_wakes", &self.clock.pending_wake_count())
            .finish()
    }
}
