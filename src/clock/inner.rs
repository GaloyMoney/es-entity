use std::sync::Arc;

use super::{artificial::ArtificialClock, realtime::RealtimeClock};

/// Internal clock implementation.
pub(crate) enum ClockInner {
    Realtime(RealtimeClock),
    Artificial(Arc<ArtificialClock>),
}
