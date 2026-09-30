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
pub use profile::{AllLanes, LaneProfile, Settled};

#[cfg(feature = "sqlx")]
pub mod sqlx;

#[cfg(feature = "tracing")]
mod record;

pub use dynamic::{denied_of, fatal_of, lane_of, transient_of};
pub use fail::{
    Fail, Failure, Fault, Laned, Level, Lift, Liftable, Rejection, RejectionField,
    RejectionMetadata, UnmappedInto, WidenResult,
};
pub use lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient, TransientKind};

#[cfg(feature = "sqlx")]
pub use sqlx::{classify_sqlx, classify_sqlx_fault, transient_sqlstate};

#[cfg(feature = "tracing")]
pub use record::{FIELDS, RecordResult, record, record_fail, record_fault};

#[cfg(feature = "derive")]
pub use errlanes_derive::{__compose_rejection, Failure, Lift, Rejection, compose};
