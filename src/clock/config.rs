use chrono::{DateTime, Utc};

/// Configuration for artificial time.
#[derive(Debug, Clone)]
pub struct ArtificialClockConfig {
    /// What time should the clock start at (truncated to millisecond precision).
    pub start_at: DateTime<Utc>,
    /// How should time advance.
    pub mode: ArtificialMode,
}

/// Truncate a DateTime to millisecond precision.
/// This ensures consistency since we store time as epoch milliseconds.
fn truncate_to_millis(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(time.timestamp_millis()).expect("valid timestamp")
}

impl ArtificialClockConfig {
    /// Create a manual config starting at the current time.
    pub fn manual() -> Self {
        Self {
            start_at: truncate_to_millis(Utc::now()),
            mode: ArtificialMode::Manual,
        }
    }

    /// Create a manual config starting at a specific time.
    pub fn manual_at(start_at: DateTime<Utc>) -> Self {
        Self {
            start_at: truncate_to_millis(start_at),
            mode: ArtificialMode::Manual,
        }
    }

    /// Create an auto-advancing config.
    pub fn auto(time_scale: f64) -> Self {
        Self {
            start_at: truncate_to_millis(Utc::now()),
            mode: ArtificialMode::AutoAdvance { time_scale },
        }
    }

    /// Create an auto-advancing config starting at a specific time.
    pub fn auto_at(start_at: DateTime<Utc>, time_scale: f64) -> Self {
        Self {
            start_at: truncate_to_millis(start_at),
            mode: ArtificialMode::AutoAdvance { time_scale },
        }
    }
}

impl Default for ArtificialClockConfig {
    fn default() -> Self {
        Self::manual()
    }
}

/// How artificial time advances.
#[derive(Debug, Clone, Copy)]
pub enum ArtificialMode {
    /// Time advances automatically at the given scale.
    /// A time_scale of 1000.0 means 1 real second = 1000 artificial seconds.
    AutoAdvance {
        /// Multiplier for time passage (e.g., 1000.0 = 1000x faster)
        time_scale: f64,
    },
    /// Time only advances via explicit `advance()` or `set_time()` calls.
    /// This is ideal for deterministic testing.
    Manual,
}
