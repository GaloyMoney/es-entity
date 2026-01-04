use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::task::Waker;
use std::time::{Duration, Instant};

use crate::config::{SimulationConfig, SimulationMode};

/// Counter for unique sleep IDs.
static NEXT_SLEEP_ID: AtomicU64 = AtomicU64::new(0);

/// Generate a unique sleep ID.
pub(crate) fn next_sleep_id() -> u64 {
    NEXT_SLEEP_ID.fetch_add(1, Ordering::Relaxed)
}

/// Simulated clock with support for auto-advance and manual modes.
pub(crate) struct SimulatedClock {
    /// Current simulated time as epoch milliseconds.
    current_ms: AtomicI64,
    /// Configuration including mode.
    config: SimulationConfig,
    /// For auto-advance mode: when simulation started in real time.
    real_start: Instant,
    /// Priority queue of pending wake events (earliest first).
    pending_wakes: Mutex<BinaryHeap<PendingWake>>,
}

/// A pending wake event in the priority queue.
pub(crate) struct PendingWake {
    /// When to wake (simulated epoch ms).
    pub wake_at_ms: i64,
    /// Unique ID for this sleep (for cancellation).
    pub sleep_id: u64,
    /// Waker to call when time arrives.
    pub waker: Waker,
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

impl SimulatedClock {
    /// Create a new simulated clock with the given configuration.
    pub fn new(config: SimulationConfig) -> Self {
        // Truncate to millisecond precision since we store as epoch millis
        let start_ms = config.start_at.timestamp_millis();
        Self {
            current_ms: AtomicI64::new(start_ms),
            real_start: Instant::now(),
            config,
            pending_wakes: Mutex::new(BinaryHeap::new()),
        }
    }

    /// Get the current simulated time.
    pub fn now(&self) -> DateTime<Utc> {
        let ms = match self.config.mode {
            SimulationMode::Manual => self.current_ms.load(Ordering::SeqCst),
            SimulationMode::AutoAdvance { time_scale } => {
                let base_ms = self.current_ms.load(Ordering::SeqCst);
                let real_elapsed = self.real_start.elapsed();
                let sim_elapsed_ms = (real_elapsed.as_millis() as f64 * time_scale) as i64;
                base_ms + sim_elapsed_ms
            }
        };
        DateTime::from_timestamp_millis(ms).expect("valid timestamp")
    }

    /// Get the current time as epoch milliseconds.
    pub fn now_ms(&self) -> i64 {
        match self.config.mode {
            SimulationMode::Manual => self.current_ms.load(Ordering::SeqCst),
            SimulationMode::AutoAdvance { time_scale } => {
                let base_ms = self.current_ms.load(Ordering::SeqCst);
                let real_elapsed = self.real_start.elapsed();
                let sim_elapsed_ms = (real_elapsed.as_millis() as f64 * time_scale) as i64;
                base_ms + sim_elapsed_ms
            }
        }
    }

    /// Convert simulated duration to real duration (for auto-advance mode).
    pub fn real_duration(&self, duration: Duration) -> Duration {
        match self.config.mode {
            SimulationMode::Manual => {
                // In manual mode, we don't use real sleeps - just register for wake
                Duration::from_millis(0)
            }
            SimulationMode::AutoAdvance { time_scale } => {
                let real_ms = (duration.as_millis() as f64 / time_scale).ceil() as u64;
                Duration::from_millis(real_ms.max(1))
            }
        }
    }

    /// Check if this is manual mode.
    pub fn is_manual(&self) -> bool {
        matches!(self.config.mode, SimulationMode::Manual)
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

    /// Cancel a pending wake event.
    pub fn cancel_wake(&self, sleep_id: u64) {
        let mut pending = self.pending_wakes.lock();
        // Rebuild heap without the cancelled entry
        let entries: Vec<_> = pending.drain().filter(|w| w.sleep_id != sleep_id).collect();
        pending.extend(entries);
    }

    /// Peek at the next wake time, if any.
    pub fn next_wake_time(&self) -> Option<i64> {
        let pending = self.pending_wakes.lock();
        pending.peek().map(|w| w.wake_at_ms)
    }

    /// Wake all tasks scheduled at or before the given time.
    /// Returns the number of tasks woken.
    pub fn wake_tasks_at(&self, up_to_ms: i64) -> usize {
        let mut pending = self.pending_wakes.lock();
        let mut count = 0;

        while let Some(wake) = pending.peek() {
            if wake.wake_at_ms > up_to_ms {
                break;
            }
            let wake = pending.pop().unwrap();
            wake.waker.wake();
            count += 1;
        }

        count
    }

    /// Set the current time directly.
    pub fn set_time(&self, time: DateTime<Utc>) {
        self.current_ms
            .store(time.timestamp_millis(), Ordering::SeqCst);
    }

    /// Set the current time to a specific millisecond value.
    pub fn set_time_ms(&self, ms: i64) {
        self.current_ms.store(ms, Ordering::SeqCst);
    }

    /// Advance time by the given duration, processing wake events in order.
    /// Returns the number of wake events processed.
    pub async fn advance(&self, duration: Duration) -> usize {
        if !self.is_manual() {
            // Auto-advance mode doesn't support explicit advance
            return 0;
        }

        let start_ms = self.current_ms.load(Ordering::SeqCst);
        let target_ms = start_ms + duration.as_millis() as i64;
        let mut total_woken = 0;

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

        total_woken
    }

    /// Advance to the next pending wake event.
    /// Returns the time advanced to, or None if no pending wakes.
    pub async fn advance_to_next_wake(&self) -> Option<DateTime<Utc>> {
        if !self.is_manual() {
            return None;
        }

        let next_wake_ms = self.next_wake_time()?;

        self.current_ms.store(next_wake_ms, Ordering::SeqCst);
        self.wake_tasks_at(next_wake_ms);
        tokio::task::yield_now().await;

        Some(DateTime::from_timestamp_millis(next_wake_ms).expect("valid timestamp"))
    }

    /// Get the number of pending wake events.
    pub fn pending_wake_count(&self) -> usize {
        self.pending_wakes.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_now() {
        let clock = SimulatedClock::new(SimulationConfig::manual());
        
        // Get the start time from the clock (already truncated to ms)
        let start = clock.now();

        // Time doesn't advance on its own
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn test_auto_advance_now() {
        let start = Utc::now();
        let clock = SimulatedClock::new(SimulationConfig::auto_at(start, 1000.0));

        let t1 = clock.now();
        std::thread::sleep(Duration::from_millis(10));
        let t2 = clock.now();

        // Should have advanced roughly 10 seconds (10ms * 1000x)
        let elapsed = t2 - t1;
        assert!(elapsed.num_seconds() >= 5 && elapsed.num_seconds() <= 20);
    }

    #[test]
    fn test_pending_wake_ordering() {
        let clock = SimulatedClock::new(SimulationConfig::manual());

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
