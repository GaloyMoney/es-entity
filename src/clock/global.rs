use chrono::{DateTime, Utc};

use std::sync::OnceLock;
use std::time::Duration;

use super::{ClockController, ClockHandle, ClockSleep, ClockTimeout};

struct GlobalState {
    handle: ClockHandle,
    controller: Option<ClockController>,
}

static GLOBAL: OnceLock<GlobalState> = OnceLock::new();

/// Global clock access - like `Utc::now()` but testable.
pub struct Clock;

impl Clock {
    /// Get current time from the global clock.
    ///
    /// Lazily initializes to realtime if not already set.
    pub fn now() -> DateTime<Utc> {
        Self::handle().now()
    }

    /// Get the current date (without time component).
    ///
    /// Lazily initializes to realtime if not already set.
    pub fn today() -> chrono::NaiveDate {
        Self::handle().today()
    }

    /// Sleep using the global clock.
    pub fn sleep(duration: Duration) -> ClockSleep {
        Self::handle().sleep(duration)
    }

    /// Sleep using the global clock with coalesceable wake-up behavior.
    ///
    /// See [`ClockHandle::sleep_coalesce`] for details.
    pub fn sleep_coalesce(duration: Duration) -> ClockSleep {
        Self::handle().sleep_coalesce(duration)
    }

    /// Timeout using the global clock.
    pub fn timeout<F: std::future::Future>(duration: Duration, future: F) -> ClockTimeout<F> {
        Self::handle().timeout(duration, future)
    }

    /// Get a reference to the global clock handle.
    pub fn handle() -> &'static ClockHandle {
        &GLOBAL
            .get_or_init(|| GlobalState {
                handle: ClockHandle::realtime(),
                controller: None,
            })
            .handle
    }

    /// Install a manual clock globally.
    ///
    /// - If not initialized: installs manual clock, returns controller
    /// - If already manual: returns existing controller (idempotent)
    /// - If already realtime: panics
    ///
    /// Must be called before any `Clock::now()` calls if you want manual time.
    pub fn install_manual() -> ClockController {
        Self::install_manual_at(Utc::now())
    }

    /// Install a manual clock globally starting at a specific time.
    ///
    /// See [`install_manual`](Self::install_manual) for details.
    pub fn install_manual_at(start_at: DateTime<Utc>) -> ClockController {
        // Check if already initialized
        if let Some(state) = GLOBAL.get() {
            return state
                .controller
                .clone()
                .expect("Cannot install manual clock: realtime clock already initialized");
        }

        // Try to initialize
        let (handle, ctrl) = ClockHandle::manual_at(start_at);

        match GLOBAL.set(GlobalState {
            handle,
            controller: Some(ctrl.clone()),
        }) {
            Ok(()) => ctrl,
            Err(_) => {
                // Race: someone else initialized between our check and set
                GLOBAL
                    .get()
                    .unwrap()
                    .controller
                    .clone()
                    .expect("Cannot install manual clock: realtime clock already initialized")
            }
        }
    }

    /// Check if a manual clock is installed.
    pub fn is_manual() -> bool {
        GLOBAL
            .get()
            .map(|s| s.controller.is_some())
            .unwrap_or(false)
    }

    /// Get the current manual time, if a manual clock is installed.
    ///
    /// Returns:
    /// - `None` if no clock is initialized (doesn't initialize one)
    /// - `None` for realtime clocks
    /// - `Some(time)` for manual clocks
    pub fn manual_now() -> Option<DateTime<Utc>> {
        GLOBAL.get().and_then(|s| s.handle.manual_now())
    }
}
