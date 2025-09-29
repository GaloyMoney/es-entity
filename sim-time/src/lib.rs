#![cfg_attr(feature = "fail-on-warnings", deny(warnings))]
#![cfg_attr(feature = "fail-on-warnings", deny(clippy::all))]
#![forbid(unsafe_code)]

mod config;

use chrono::{DateTime, Utc};
pub use config::*;
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

static INSTANCE: OnceLock<Time> = OnceLock::new();

#[derive(Clone)]
struct Time {
    config: TimeConfig,
    elapsed_ms: Arc<AtomicU64>,
    ticker_task: Arc<OnceLock<()>>,
}

impl Time {
    fn new(config: TimeConfig) -> Self {
        let time = Self {
            config,
            elapsed_ms: Arc::new(AtomicU64::new(0)),
            ticker_task: Arc::new(OnceLock::new()),
        };
        if !time.config.realtime {
            time.spawn_ticker();
        }
        time
    }

    fn spawn_ticker(&self) {
        let elapsed_ms = self.elapsed_ms.clone();
        let sim_config = &self.config.simulation;
        let tick_interval_ms = sim_config.tick_interval_ms;
        let tick_duration = sim_config.tick_duration_secs;
        self.ticker_task.get_or_init(|| {
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(tokio::time::Duration::from_millis(tick_interval_ms));
                loop {
                    interval.tick().await;
                    elapsed_ms.fetch_add(tick_duration.as_millis() as u64, Ordering::Relaxed);
                }
            });
        });
    }

    fn now(&self) -> DateTime<Utc> {
        if self.config.realtime {
            Utc::now()
        } else {
            let sim_config = &self.config.simulation;
            let elapsed_ms = self.elapsed_ms.load(Ordering::Relaxed);

            let simulated_time =
                sim_config.start_at + chrono::Duration::milliseconds(elapsed_ms as i64);

            if sim_config.transform_to_realtime && simulated_time >= Utc::now() {
                Utc::now()
            } else {
                simulated_time
            }
        }
    }

    fn real_ms(&self, duration: Duration) -> Duration {
        if self.config.realtime {
            duration
        } else {
            let sim_config = &self.config.simulation;

            let current_time = self.now();
            let real_now = Utc::now();

            if sim_config.transform_to_realtime && current_time >= real_now {
                return duration;
            }

            let sim_ms_per_real_ms = sim_config.tick_duration_secs.as_millis() as f64
                / sim_config.tick_interval_ms as f64;

            Duration::from_millis((duration.as_millis() as f64 / sim_ms_per_real_ms).ceil() as u64)
        }
    }

    fn sleep(&self, duration: Duration) -> tokio::time::Sleep {
        tokio::time::sleep(self.real_ms(duration))
    }

    fn timeout<F>(&self, duration: Duration, future: F) -> tokio::time::Timeout<F::IntoFuture>
    where
        F: core::future::IntoFuture,
    {
        tokio::time::timeout(self.real_ms(duration), future)
    }

    pub async fn wait_until_realtime(&self) {
        if self.config.realtime {
            return;
        }

        let current = self.now();
        let real_now = Utc::now();

        if current >= real_now {
            return;
        }

        let wait_duration =
            std::time::Duration::from_millis((real_now - current).num_milliseconds() as u64);

        self.sleep(wait_duration).await;
    }
}

/// Returns a future that will return when the simulation has caught up to the current time.
///
/// Assumes that the simulation was configured to start in the past and has
/// [`SimulationConfig::transform_to_realtime`](`config::SimulationConfig::transform_to_realtime`) set to `true`.
pub async fn wait_until_realtime() {
    INSTANCE
        .get_or_init(|| Time::new(TimeConfig::default()))
        .wait_until_realtime()
        .await
}

/// Pass the [`TimeConfig`] to configure `sim-time` globally.
/// Must be called before any other `fn`s otherwise `sim-time` will initialize with defaults.
pub fn init(config: TimeConfig) {
    INSTANCE.get_or_init(|| Time::new(config));
}

/// Returns the current time in the simulation
pub fn now() -> DateTime<Utc> {
    INSTANCE
        .get_or_init(|| Time::new(TimeConfig::default()))
        .now()
}

/// Will sleep for the simulated duration.
pub fn sleep(duration: Duration) -> tokio::time::Sleep {
    INSTANCE
        .get_or_init(|| Time::new(TimeConfig::default()))
        .sleep(duration)
}

/// Will timeout for the simulated duration.
pub fn timeout<F>(duration: Duration, future: F) -> tokio::time::Timeout<F::IntoFuture>
where
    F: core::future::IntoFuture,
{
    INSTANCE
        .get_or_init(|| Time::new(TimeConfig::default()))
        .timeout(duration, future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use std::time::Duration as StdDuration;

    #[tokio::test]
    async fn test_simulated_time() {
        // Configure time where 10ms = 10 days of simulated time
        let config = TimeConfig {
            realtime: false,
            simulation: SimulationConfig {
                start_at: Utc::now(),
                tick_interval_ms: 10,
                tick_duration_secs: StdDuration::from_secs(10 * 24 * 60 * 60), // 10 days in seconds
                transform_to_realtime: false,
            },
        };

        init(config);
        let start = now();
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        let end = now();
        let elapsed = end - start;

        assert!(
            elapsed >= ChronoDuration::days(19) && elapsed <= ChronoDuration::days(21),
            "Expected ~20 days to pass, but got {} days",
            elapsed.num_days()
        );
    }

    #[test]
    fn test_default_realtime() {
        let t1 = now();
        let t2 = Utc::now();
        assert!(t2 - t1 < ChronoDuration::seconds(1));
    }
}
