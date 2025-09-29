use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

/// Top level configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeConfig {
    /// Setting `realtime: true` deactivates sim-time.
    /// Only if its set to `false` will the [`SimulationConfig`] take effect.
    pub realtime: bool,
    /// Configuration of how the simulation should behave
    #[serde(default)]
    pub simulation: SimulationConfig,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            realtime: true,
            simulation: SimulationConfig::default(),
        }
    }
}

/// Configuration of how the simulation should behave.
#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SimulationConfig {
    /// What date should the simulation start at.
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

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            start_at: chrono::DateTime::parse_from_rfc3339("2021-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            tick_interval_ms: 1,
            tick_duration_secs: std::time::Duration::from_secs(1000),
            transform_to_realtime: false,
        }
    }
}
