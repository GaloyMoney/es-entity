use std::sync::Arc;

use crate::realtime::RealtimeClock;
use crate::simulated::SimulatedClock;

/// Internal clock implementation.
pub(crate) enum ClockInner {
    Realtime(RealtimeClock),
    Simulated(Arc<SimulatedClock>),
}
