use chrono::{DateTime, Utc};

use std::{sync::Arc, time::Duration};

use super::manual::ManualClock;

/// Controller for manual time operations.
///
/// This is only available for manual clocks and provides methods to
/// advance time and inspect pending wake events.
///
/// Created alongside a [`ClockHandle`](crate::ClockHandle) via
/// [`ClockHandle::manual()`](crate::ClockHandle::manual).
#[derive(Clone)]
pub struct ClockController {
    pub(crate) clock: Arc<ManualClock>,
}

impl ClockController {
    /// Advance time by the given duration.
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
    /// use es_entity::clock::ClockHandle;
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let (clock, ctrl) = ClockHandle::manual();
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
    /// use es_entity::clock::ClockHandle;
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let (clock, ctrl) = ClockHandle::manual();
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

    /// Get the number of pending wake events.
    ///
    /// This is useful for testing to verify that tasks have registered
    /// their sleeps before advancing time.
    pub fn pending_wake_count(&self) -> usize {
        self.clock.pending_wake_count()
    }

    /// Get the current time.
    ///
    /// This is equivalent to calling `now()` on the associated `ClockHandle`.
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
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
