//! Four error lanes — `Rejected(D)` / `Denied` / `Transient` / `Fatal` —
//! carried in the type instead of re-derived at every layer.
//!
//! See the crate [README](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/README.md)
//! for a step-by-step introduction with runnable examples.

#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod dynamic;
mod fail;
mod lane;
pub mod profile;
pub use profile::{AllLanes, LaneProfile};

#[cfg(feature = "sqlx")]
pub mod sqlx;

#[cfg(feature = "tokio")]
mod retry;

#[cfg(feature = "tracing")]
mod record;

pub use dynamic::{Classify, lane_of, transient_of};
pub use fail::{
    ExhaustionInto, Fail, Failure, Fault, Laned, Level, Lift, LiftResult, Liftable, Rejection,
    RejectionField, RejectionMetadata, Settled, SettledFault, UnmappedInto, WidenResult,
};
pub use lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient, TransientKind};

#[cfg(feature = "sqlx")]
pub use sqlx::{classify_sqlx, classify_sqlx_fault, lane_of_sqlx, transient_sqlstate};

#[cfg(feature = "tokio")]
pub use retry::{RetryPolicy, retry, retry_with};

#[cfg(feature = "tracing")]
pub use record::{FIELDS, record, record_fail, record_fault, record_settled_fault};

#[cfg(feature = "derive")]
pub use errlanes_derive::{__compose_rejection, Classify, Failure, Lift, Rejection, compose};
