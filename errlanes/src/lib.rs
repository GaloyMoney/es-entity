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
pub use profile::{AllLanes, LaneProfile, WithoutDenied, WithoutTransient};

#[cfg(feature = "sqlx")]
pub mod sqlx;

#[cfg(feature = "tracing")]
mod record;

pub use fail::{
    Fail, Failure, Fault, Laned, Level, Lift, Rejection, RejectionField, RejectionMetadata,
    UnmappedInto, WidenResult,
};
pub use lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient, TransientKind};

#[cfg(feature = "tracing")]
pub use record::{FIELDS, RecordResult};

#[cfg(feature = "derive")]
pub use errlanes_derive::{__compose_rejection, Failure, Lift, Rejection, compose};

#[cfg(all(feature = "derive", feature = "tracing"))]
pub use errlanes_derive::instrument;
