use chrono::{DateTime, Utc};

/// Configuration for simulated time.
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    /// What time should the simulation start at (truncated to millisecond precision).
    pub start_at: DateTime<Utc>,
    /// How should time advance.
    pub mode: SimulationMode,
}

/// Truncate a DateTime to millisecond precision.
/// This ensures consistency since we store time as epoch milliseconds.
fn truncate_to_millis(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(time.timestamp_millis()).expect("valid timestamp")
}

impl SimulationConfig {
    /// Create a manual simulation config starting at the current time.
    pub fn manual() -> Self {
        Self {
            start_at: truncate_to_millis(Utc::now()),
            mode: SimulationMode::Manual,
        }
    }

    /// Create a manual simulation config starting at a specific time.
    pub fn manual_at(start_at: DateTime<Utc>) -> Self {
        Self {
            start_at: truncate_to_millis(start_at),
            mode: SimulationMode::Manual,
        }
    }

    /// Create an auto-advancing simulation config.
    pub fn auto(time_scale: f64) -> Self {
        Self {
            start_at: truncate_to_millis(Utc::now()),
            mode: SimulationMode::AutoAdvance { time_scale },
        }
    }

    /// Create an auto-advancing simulation starting at a specific time.
    pub fn auto_at(start_at: DateTime<Utc>, time_scale: f64) -> Self {
        Self {
            start_at: truncate_to_millis(start_at),
            mode: SimulationMode::AutoAdvance { time_scale },
        }
    }
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self::manual()
    }
}

/// How simulated time advances.
#[derive(Debug, Clone, Copy)]
pub enum SimulationMode {
    /// Time advances automatically at the given scale.
    /// A time_scale of 1000.0 means 1 real second = 1000 simulated seconds.
    AutoAdvance {
        /// Multiplier for time passage (e.g., 1000.0 = 1000x faster)
        time_scale: f64,
    },
    /// Time only advances via explicit `advance()` or `set_time()` calls.
    /// This is ideal for deterministic testing.
    Manual,
}
