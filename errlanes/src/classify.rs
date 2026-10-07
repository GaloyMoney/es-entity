//! How a local error enters the lanes: which part is a typed domain outcome
//! ([`Fail::Rejected`]) and which fault lanes the rest can take.
//!
//! A foreign error (`sqlx::Error`, `serde_json::Error`, `reqwest::Error`, …)
//! never implements [`Classify`] itself unless errlanes blesses it behind a
//! `classify-*` feature (`sqlx.rs`, `serde_json.rs`, `reqwest.rs`) — a
//! consumer wraps it in a local type that does, via
//! `#[derive(errlanes::Classify)]` or by hand, and enters with
//! `.classify::<Wrapper>()?`.

use std::{convert::Infallible, error::Error};

use crate::{
    fail::{Fail, Rejection},
    profile::LaneProfile,
};

/// The rejected slot of a [`Classify`] type: a [`Rejection`], or `Infallible`
/// for a type that never rejects — the same encoding a disabled lane slot
/// uses.
pub trait RejectedSlot: Send + Sync + 'static {}
impl RejectedSlot for Infallible {}
impl<R: Rejection> RejectedSlot for R {}

/// The pairwise union of two rejected slots — `derive(Classify)` folds a
/// mixed wrapper's `Rejected` this way across its `delegate` variants,
/// order-independent: at most one of the two may be a genuine `Rejection`
/// (the other `Infallible`), so there is always exactly one sensible `Out`.
/// Two genuine, *different* `Rejection`s have no impl here at all — that is
/// the "a mixed wrapper rejects through one type; compose them" rule,
/// enforced by the type system rather than by the derive trying to read
/// ahead across fields it cannot resolve the types of.
#[doc(hidden)]
pub trait RejectedUnion<Other: RejectedSlot>: RejectedSlot {
    type Out: RejectedSlot;
}
impl RejectedUnion<Infallible> for Infallible {
    type Out = Infallible;
}
impl<R: Rejection> RejectedUnion<Infallible> for R {
    type Out = R;
}
impl<R: Rejection> RejectedUnion<R> for Infallible {
    type Out = R;
}
impl<R: Rejection> RejectedUnion<R> for R {
    type Out = R;
}

/// How a local error enters the lanes: which part is a typed domain outcome
/// that a caller can correct, and which fault lanes the rest can take.
///
/// A pure [`Rejection`] gets this for free (the blanket below): all of it is
/// rejected, no lanes. A fault wrapper or a mixed wrapper implements it
/// directly, by hand or via `#[derive(errlanes::Classify)]`. A type is one or
/// the other, never both — `Rejection` and a direct `Classify` impl on the
/// same type conflict (`E0119`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not say how it enters the lanes",
    note = "derive `errlanes::Rejection` if a caller can correct it, or `errlanes::Classify` \
            with a lane (`#[classify(fatal(Kind))]`, …); a foreign error is wrapped in a \
            local type first"
)]
pub trait Classify: Error + Send + Sync + 'static {
    type Rejected: RejectedSlot;
    type Lanes: LaneProfile;

    fn classify(self) -> Fail<Self::Rejected, Self::Lanes>;
}

/// A [`Rejection`] is the special case of [`Classify`]: all of it is
/// rejected, no lanes.
impl<R: Rejection> Classify for R {
    type Rejected = R;
    type Lanes = crate::profile::NoLanes;

    fn classify(self) -> Fail<R, Self::Lanes> {
        Fail::Rejected(self)
    }
}
