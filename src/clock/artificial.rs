use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use std::{
    cmp::Ordering as CmpOrdering,
    collections::BinaryHeap,
    sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering},
    task::Waker,
    time::{Duration, Instant},
};

use super::config::{SimulationConfig, SimulationMode};

/// Counter for unique sleep IDs.
static NEXT_SLEEP_ID: AtomicU64 = AtomicU64::new(0);

/// Generate a unique sleep ID.
pub(crate) fn next_sleep_id() -> u64 {
    NEXT_SLEEP_ID.fetch_add(1, Ordering::Relaxed)
}

// Mode constants for atomic storage
const MODE_MANUAL: u8 = 0;
const MODE_AUTO: u8 = 1;
const MODE_REALTIME: u8 = 2;

/// Artificial clock with support for auto-advance, manual, and realtime modes.
pub(crate) struct ArtificialClock {
    /// Current mode (manual, auto, or realtime).
    mode: AtomicU8,
    /// Time scale for auto-advance mode.
    time_scale: f64,
    /// Current artificial time as epoch milliseconds (used in manual/auto modes).
    current_ms: AtomicI64,
    /// For auto-advance mode: when simulation started in real time.
    real_start: Instant,
    /// Priority queue of pending wake events (earliest first).
    pending_wakes: Mutex<BinaryHeap<PendingWake>>,
}

/// A pending wake event in the priority queue.
pub(crate) struct PendingWake {
    /// When to wake (artificial epoch ms).
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

impl ArtificialClock {
    /// Create a new artificial clock with the given configuration.
    pub fn new(config: SimulationConfig) -> Self {
        let (mode, time_scale) = match config.mode {
            SimulationMode::Manual => (MODE_MANUAL, 0.0),
            SimulationMode::AutoAdvance { time_scale } => (MODE_AUTO, time_scale),
        };

        Self {
            mode: AtomicU8::new(mode),
            time_scale,
            current_ms: AtomicI64::new(config.start_at.timestamp_millis()),
            real_start: Instant::now(),
            pending_wakes: Mutex::new(BinaryHeap::new()),
        }
    }

    /// Get the current artificial time.
    pub fn now(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.now_ms()).expect("valid timestamp")
    }

    /// Get the current time as epoch milliseconds.
    pub fn now_ms(&self) -> i64 {
        match self.mode.load(Ordering::Acquire) {
            MODE_REALTIME => Utc::now().timestamp_millis(),
            MODE_MANUAL => self.current_ms.load(Ordering::SeqCst),
            MODE_AUTO => {
                let base_ms = self.current_ms.load(Ordering::SeqCst);
                let real_elapsed = self.real_start.elapsed();
                base_ms + (real_elapsed.as_millis() as f64 * self.time_scale) as i64
            }
            _ => unreachable!(),
        }
    }

    /// Convert artificial duration to real duration.
    pub(crate) fn real_duration(&self, duration: Duration) -> Duration {
        match self.mode.load(Ordering::Acquire) {
            MODE_REALTIME => duration,
            MODE_MANUAL => Duration::ZERO,
            MODE_AUTO => {
                let real_ms = (duration.as_millis() as f64 / self.time_scale).ceil() as u64;
                Duration::from_millis(real_ms.max(1))
            }
            _ => unreachable!(),
        }
    }

    /// Check if this is manual mode.
    pub fn is_manual(&self) -> bool {
        self.mode.load(Ordering::Acquire) == MODE_MANUAL
    }

    /// Check if this has transitioned to realtime.
    pub fn is_realtime(&self) -> bool {
        self.mode.load(Ordering::Acquire) == MODE_REALTIME
    }

    /// Transition to realtime mode.
    ///
    /// After this call, `now()` returns `Utc::now()` and sleeps use real tokio timers.
    pub fn transition_to_realtime(&self) {
        self.mode.store(MODE_REALTIME, Ordering::Release);
        self.wake_all_pending();
    }

    /// Wake all pending tasks.
    fn wake_all_pending(&self) {
        let mut pending = self.pending_wakes.lock();
        for wake in pending.drain() {
            wake.waker.wake();
        }
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

    /// Clear all pending wake events.
    pub fn clear_pending_wakes(&self) {
        self.pending_wakes.lock().clear();
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

    /// Advance time by the given duration, processing wake events in order.
    /// Returns the number of wake events processed.
    pub async fn advance(&self, duration: Duration) -> usize {
        if !self.is_manual() {
            // Auto-advance and realtime modes don't support explicit advance
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
        let clock = ArtificialClock::new(SimulationConfig::manual());

        let start = clock.now();

        // Time doesn't advance on its own
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(clock.now(), start);
    }

    #[test]
    fn test_auto_advance_now() {
        let start = Utc::now();
        let clock = ArtificialClock::new(SimulationConfig::auto_at(start, 1000.0));

        let t1 = clock.now();
        std::thread::sleep(Duration::from_millis(10));
        let t2 = clock.now();

        // Should have advanced roughly 10 seconds (10ms * 1000x)
        let elapsed = t2 - t1;
        assert!(elapsed.num_seconds() >= 5 && elapsed.num_seconds() <= 20);
    }

    #[test]
    fn test_transition_to_realtime() {
        let clock = ArtificialClock::new(SimulationConfig::manual());

        assert!(clock.is_manual());
        assert!(!clock.is_realtime());

        clock.transition_to_realtime();

        assert!(!clock.is_manual());
        assert!(clock.is_realtime());

        // now() should return approximately Utc::now()
        let clock_now = clock.now();
        let utc_now = Utc::now();
        let diff = (clock_now - utc_now).num_milliseconds().abs();
        assert!(diff < 100); // Within 100ms
    }

    #[test]
    fn test_pending_wake_ordering() {
        let clock = ArtificialClock::new(SimulationConfig::manual());

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

    #[test]
    fn test_clear_pending_wakes() {
        let clock = ArtificialClock::new(SimulationConfig::manual());
        let waker = futures::task::noop_waker();

        clock.register_wake(1000, 1, waker.clone());
        clock.register_wake(2000, 2, waker);

        assert_eq!(clock.pending_wake_count(), 2);

        clock.clear_pending_wakes();

        assert_eq!(clock.pending_wake_count(), 0);
    }
}
