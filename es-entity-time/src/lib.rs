//! Time abstraction for es-entity with support for real and simulated time.
//!
//! This crate provides a unified interface for time operations that works
//! identically whether using real time or simulated time for testing.
//!
//! # Overview
//!
//! The main type is [`ClockHandle`], a cheap-to-clone handle that provides:
//! - `now()` - Get current time (synchronous, fast)
//! - `sleep(duration)` - Sleep for a duration
//! - `timeout(duration, future)` - Timeout a future
//!
//! For simulated clocks, a [`ClockController`] is also provided for controlling time.
//!
//! # Clock Types
//!
//! - **Realtime**: Uses system clock and tokio timers
//! - **Simulated (Auto)**: Time advances automatically at a configurable scale
//! - **Simulated (Manual)**: Time only advances via explicit `advance()` calls
//!
//! # Example
//!
//! ```rust
//! use es_entity_time::{ClockHandle, SimulationConfig};
//! use std::time::Duration;
//!
//! // Production: use real time
//! let clock = ClockHandle::realtime();
//!
//! // Testing: use manual simulation
//! let (clock, ctrl) = ClockHandle::simulated(SimulationConfig::manual());
//!
//! // Same interface regardless of clock type
//! let now = clock.now();
//! ```
//!
//! # Deterministic Testing
//!
//! In manual simulation mode, time only advances when you call `advance()`.
//! Wake events are processed in chronological order, so tasks always see
//! the correct time when they wake:
//!
//! ```rust,no_run
//! use es_entity_time::{ClockHandle, SimulationConfig};
//! use std::time::Duration;
//!
//! # async fn example() {
//! let (clock, ctrl) = ClockHandle::simulated(SimulationConfig::manual());
//!
//! let clock2 = clock.clone();
//! tokio::spawn(async move {
//!     clock2.sleep(Duration::from_secs(3600)).await; // 1 hour
//!     // When this wakes, clock2.now() == start + 1 hour
//!     // (even if advance() jumped further)
//! });
//!
//! // Advance 1 day - but the task wakes at exactly +1 hour
//! ctrl.advance(Duration::from_secs(86400)).await;
//! # }
//! ```

#![cfg_attr(feature = "fail-on-warnings", deny(warnings))]
#![cfg_attr(feature = "fail-on-warnings", deny(clippy::all))]
#![forbid(unsafe_code)]

mod config;
mod controller;
mod handle;
mod inner;
mod realtime;
mod simulated;
mod sleep;
#[cfg(feature = "sqlx")]
mod transaction;

pub use config::*;
pub use controller::*;
pub use handle::*;
#[cfg(feature = "sqlx")]
pub use transaction::*;
