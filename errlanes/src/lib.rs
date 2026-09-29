//! Four error lanes — `Rejected(D)` / `Denied` / `Transient` / `Fatal` —
//! carried in the type instead of re-derived at every layer.
//!
//! See the crate [README](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/README.md)
//! for the adoption tiers and a worked example.

#![forbid(unsafe_code)]

mod dynamic;
mod fail;
mod lane;

#[cfg(feature = "sqlx")]
pub mod sqlx;

#[cfg(feature = "tokio")]
mod retry;

#[cfg(feature = "tracing")]
mod record;

pub use dynamic::{Classify, lane_of, transient_of};
pub use fail::{
    Fail, Failure, HasConstraint, Level, LiftConstraint, NeverCode, Rejection, Settled,
};
pub use lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient, TransientKind};

#[cfg(feature = "sqlx")]
pub use sqlx::{classify_sqlx, lane_of_sqlx, transient_sqlstate};

#[cfg(feature = "tokio")]
pub use retry::{RetryPolicy, retry, retry_with};

#[cfg(feature = "tracing")]
pub use record::{FIELDS, record, record_fail};

#[cfg(feature = "derive")]
pub use errlanes_derive::{Classify, Failure, Rejection};
