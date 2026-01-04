use std::sync::Arc;

use crate::realtime::RealtimeClock;
use crate::artificial::ArtificialClock;

/// Internal clock implementation.
pub(crate) enum ClockInner {
    Realtime(RealtimeClock),
    Artificial(Arc<ArtificialClock>),
}
