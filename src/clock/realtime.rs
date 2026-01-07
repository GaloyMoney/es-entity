use chrono::{DateTime, Utc};
use tokio::time::Sleep;

use std::time::Duration;

/// Real-time clock implementation using system time and tokio timers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RealtimeClock;

impl RealtimeClock {
    #[inline]
    pub fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    #[inline]
    pub fn sleep(&self, duration: Duration) -> Sleep {
        tokio::time::sleep(duration)
    }
}
