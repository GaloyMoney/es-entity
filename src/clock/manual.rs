use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use std::{
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    sync::atomic::{AtomicI64, Ordering},
    task::Waker,
    time::Duration,
};

/// Truncate a DateTime to millisecond precision.
/// This ensures consistency since we store time as epoch milliseconds.
fn truncate_to_millis(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(time.timestamp_millis()).expect("valid timestamp")
}

/// Manual clock where time only advances via explicit controller calls.
pub(crate) struct ManualClock {
    /// Current time as epoch milliseconds.
    current_ms: AtomicI64,
    /// Priority queue of pending wake events (earliest first).
    pending_wakes: Mutex<BinaryHeap<PendingWake>>,
    /// Coalesceable wakes — processed once at end of advance(), not at intermediate boundaries.
    coalesce_wakes: Mutex<Vec<PendingWake>>,
}

/// A pending wake event in the priority queue.
pub(crate) struct PendingWake {
    /// When to wake (epoch ms).
    wake_at_ms: i64,
    /// Unique ID for this sleep (for cancellation).
    sleep_id: u64,
    /// Waker to call when time arrives.
    waker: Waker,
}

// BinaryHeap is a max-heap, so we reverse the ordering to get a min-heap.
impl PartialEq for PendingWake {
    fn eq(&self, other: &Self) -> bool {
        self.wake_at_ms == other.wake_at_ms && self.sleep_id == other.sleep_id
    }
}

impl Eq for PendingWake {}

impl PartialOrd for PendingWake {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingWake {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // Reverse ordering for min-heap behavior
        match other.wake_at_ms.cmp(&self.wake_at_ms) {
            CmpOrdering::Equal => other.sleep_id.cmp(&self.sleep_id),
            ord => ord,
        }
    }
}

impl ManualClock {
    /// Create a new manual clock starting at the current time.
    pub fn new() -> Self {
        Self::new_at(Utc::now())
    }

    /// Create a new manual clock starting at a specific time.
    pub fn new_at(start_at: DateTime<Utc>) -> Self {
        Self {
            current_ms: AtomicI64::new(truncate_to_millis(start_at).timestamp_millis()),
            pending_wakes: Mutex::new(BinaryHeap::new()),
            coalesce_wakes: Mutex::new(Vec::new()),
        }
    }

    /// Get the current time.
    pub fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.now_ms()).expect("valid timestamp")
    }

    /// Get the current time as epoch milliseconds.
    pub fn now_ms(&self) -> i64 {
        self.current_ms.load(Ordering::SeqCst)
    }

    /// Register a pending wake event.
    pub fn register_wake(&self, wake_at_ms: i64, sleep_id: u64, waker: Waker) {
        let mut pending = self.pending_wakes.lock();
        pending.push(PendingWake {
            wake_at_ms,
            sleep_id,
            waker,
        });
    }

    /// Register a coalesceable wake event.
    ///
    /// Unlike regular wakes, coalesceable wakes are processed once at the end
    /// of `advance()` rather than at every intermediate boundary.
    pub fn register_coalesce_wake(&self, wake_at_ms: i64, sleep_id: u64, waker: Waker) {
        let mut coalesce = self.coalesce_wakes.lock();
        coalesce.push(PendingWake {
            wake_at_ms,
            sleep_id,
            waker,
        });
    }

    /// Cancel a pending wake event (searches both regular and coalesceable lists).
    pub fn cancel_wake(&self, sleep_id: u64) {
        {
            let mut pending = self.pending_wakes.lock();
            // Rebuild heap without the cancelled entry
            let entries: Vec<_> = pending.drain().filter(|w| w.sleep_id != sleep_id).collect();
            pending.extend(entries);
        }
        {
            let mut coalesce = self.coalesce_wakes.lock();
            coalesce.retain(|w| w.sleep_id != sleep_id);
        }
    }

    /// Peek at the next wake time, if any.
    pub fn next_wake_time(&self) -> Option<i64> {
        let pending = self.pending_wakes.lock();
        pending.peek().map(|w| w.wake_at_ms)
    }

    /// Wake all tasks scheduled at or before the given time.
    /// Returns the number of tasks woken.
    pub fn wake_tasks_at(&self, up_to_ms: i64) -> usize {
        // Collect wakers while holding the lock, then wake after releasing.
        // This avoids potential deadlock if a woken task tries to re-acquire the lock.
        let wakers: Vec<Waker> = {
            let mut pending = self.pending_wakes.lock();
            let mut wakers = Vec::new();

            while let Some(wake) = pending.peek() {
                if wake.wake_at_ms > up_to_ms {
                    break;
                }
                let wake = pending.pop().unwrap();
                wakers.push(wake.waker);
            }

            wakers
        };

        let count = wakers.len();
        for waker in wakers {
            waker.wake();
        }

        count
    }

    /// Advance time by the given duration, processing wake events in order.
    ///
    /// Regular wakes are processed at each intermediate boundary (existing behavior).
    /// Coalesceable wakes are deferred and processed once at the end of the advance.
    ///
    /// Returns the number of wake events processed.
    pub async fn advance(&self, duration: Duration) -> usize {
        let start_ms = self.current_ms.load(Ordering::SeqCst);
        let target_ms = start_ms + duration.as_millis() as i64;
        let mut total_woken = 0;

        // Process regular wakes at intermediate boundaries
        loop {
            let next_wake_ms = self.next_wake_time();

            match next_wake_ms {
                Some(wake_ms) if wake_ms <= target_ms => {
                    // Advance time to this wake point
                    self.current_ms.store(wake_ms, Ordering::SeqCst);

                    // Wake all tasks scheduled for exactly this time
                    let woken = self.wake_tasks_at(wake_ms);
                    total_woken += woken;

                    // Yield to let woken tasks run
                    tokio::task::yield_now().await;
                }
                _ => {
                    // No more wakes before target, jump to target
                    self.current_ms.store(target_ms, Ordering::SeqCst);
                    break;
                }
            }
        }

        // Process coalesceable wakes once at target time
        let coalesce_woken = self.wake_coalesce_tasks_at(target_ms);
        if coalesce_woken > 0 {
            total_woken += coalesce_woken;
            tokio::task::yield_now().await;
        }

        total_woken
    }

    /// Advance to the next pending wake event (considers both regular and coalesceable).
    /// Returns the time advanced to, or None if no pending wakes.
    pub async fn advance_to_next_wake(&self) -> Option<DateTime<Utc>> {
        let next_regular = self.next_wake_time();
        let next_coalesce = self.next_coalesce_wake_time();

        let next_wake_ms = match (next_regular, next_coalesce) {
            (Some(r), Some(c)) => Some(r.min(c)),
            (Some(r), None) => Some(r),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        }?;

        self.current_ms.store(next_wake_ms, Ordering::SeqCst);
        self.wake_tasks_at(next_wake_ms);
        self.wake_coalesce_tasks_at(next_wake_ms);
        tokio::task::yield_now().await;

        Some(DateTime::from_timestamp_millis(next_wake_ms).expect("valid timestamp"))
    }

    /// Wake all coalesceable tasks scheduled at or before the given time.
    /// Returns the number of tasks woken.
    pub fn wake_coalesce_tasks_at(&self, up_to_ms: i64) -> usize {
        // Collect wakers while holding the lock, then wake after releasing.
        let wakers: Vec<Waker> = {
            let mut coalesce = self.coalesce_wakes.lock();
            let mut wakers = Vec::new();
            let mut remaining = Vec::new();

            for wake in coalesce.drain(..) {
                if wake.wake_at_ms <= up_to_ms {
                    wakers.push(wake.waker);
                } else {
                    remaining.push(wake);
                }
            }

            *coalesce = remaining;
            wakers
        };

        let count = wakers.len();
        for waker in wakers {
            waker.wake();
        }

        count
    }

    /// Peek at the earliest coalesceable wake time, if any.
    fn next_coalesce_wake_time(&self) -> Option<i64> {
        let coalesce = self.coalesce_wakes.lock();
        coalesce.iter().map(|w| w.wake_at_ms).min()
    }

    /// Get the number of pending wake events (both regular and coalesceable).
    pub fn pending_wake_count(&self) -> usize {
        self.pending_wakes.lock().len() + self.coalesce_wakes.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_now() {
        let clock = ManualClock::new();

        let start = clock.now();

        // Time doesn't advance on its own
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn test_pending_wake_ordering() {
        let clock = ManualClock::new();

        let waker = futures::task::noop_waker();

        // Register wakes out of order
        clock.register_wake(3000, 1, waker.clone());
        clock.register_wake(1000, 2, waker.clone());
        clock.register_wake(2000, 3, waker);

        // Should process in order
        assert_eq!(clock.next_wake_time(), Some(1000));
        clock.wake_tasks_at(1000);

        assert_eq!(clock.next_wake_time(), Some(2000));
        clock.wake_tasks_at(2000);

        assert_eq!(clock.next_wake_time(), Some(3000));
    }
}
