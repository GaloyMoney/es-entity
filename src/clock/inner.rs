use std::sync::Arc;

use super::{manual::ManualClock, realtime::RealtimeClock};

/// Internal clock implementation.
pub(crate) enum ClockInner {
    Realtime(RealtimeClock),
    Manual(Arc<ManualClock>),
}
