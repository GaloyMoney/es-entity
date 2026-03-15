use serde::{Deserialize, Serialize};

use chrono::{DateTime, Utc};

/// Configuration for artificial time.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

}

impl Default for ArtificialClockConfig {
    fn default() -> Self {
        Self::manual()
    }
}

/// How artificial time advances.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtificialMode {
    /// Time only advances via explicit `advance()` or `set_time()` calls.
    /// This is ideal for deterministic testing.
    Manual,
}
