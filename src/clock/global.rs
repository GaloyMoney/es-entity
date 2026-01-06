use chrono::{DateTime, Utc};

use std::sync::OnceLock;
use std::time::Duration;

use super::{ArtificialClockConfig, ClockController, ClockHandle, ClockSleep, ClockTimeout};

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

    /// Sleep using the global clock.
    pub fn sleep(duration: Duration) -> ClockSleep {
        Self::handle().sleep(duration)
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

    /// Install an artificial clock globally.
    ///
    /// - If not initialized: installs artificial clock, returns controller
    /// - If already artificial: returns existing controller (idempotent)
    /// - If already realtime: panics
    ///
    /// Must be called before any `Clock::now()` calls if you want artificial time.
    pub fn install_artificial(config: ArtificialClockConfig) -> ClockController {
        // Check if already initialized
        if let Some(state) = GLOBAL.get() {
            return state
                .controller
                .clone()
                .expect("Cannot install artificial clock: realtime clock already initialized");
        }

        // Try to initialize
        let (handle, ctrl) = ClockHandle::artificial(config);

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
                    .expect("Cannot install artificial clock: realtime clock already initialized")
            }
        }
    }

    /// Check if an artificial clock is installed.
    pub fn is_artificial() -> bool {
        GLOBAL
            .get()
            .map(|s| s.controller.is_some())
            .unwrap_or(false)
    }
}
