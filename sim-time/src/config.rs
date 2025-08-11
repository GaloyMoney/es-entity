use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// Configuration of sim-time
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeConfig {
    /// Setting `realtime: true` deactivates sim-time.
    /// Only if its set to `false` will the [`SimTimeConfig`] take effect.
    pub realtime: bool,
    /// Configuration of how the simulation should behave
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_time: Option<SimTimeConfig>,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            realtime: true,
            sim_time: None,
        }
    }
}

/// Configuration of how time simulation should behave.
///
/// ```rust
/// ```
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SimTimeConfig {
    /// What date should the simulation start.
    #[serde(default = "Utc::now")]
    pub start_at: DateTime<Utc>,
    /// How long between 'ticks' of the simulation (in real milliseconds).
    pub tick_interval_ms: u64,
    /// How many simulated seconds does one tick represent.
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    pub tick_duration_secs: std::time::Duration,
    /// Should the simulation transition to real time when it has reached the current time.
    #[serde(default)]
    pub transform_to_realtime: bool,
}
