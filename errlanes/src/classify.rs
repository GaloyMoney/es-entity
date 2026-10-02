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
    fail::{Fail, Fault, Lift, Rejection, UnmappedInto},
    lane::{Fatal, Transient},
    profile::{LaneProfile, Profile},
};

/// The rejected slot of a [`Classify`] type: a [`Rejection`], or `Infallible`
/// for a type that never rejects — the same encoding a disabled lane slot
/// uses.
pub trait RejectedSlot: Send + Sync + 'static {}
impl RejectedSlot for Infallible {}
impl<R: Rejection> RejectedSlot for R {}

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

/// `?` into a [`Fail`]: the rejected part must be absorbed totally by `D` (no
/// decision left for the call site to make with `.widen()`); the wrapper's
/// lanes must fit the destination's profile. Replaces the narrower
/// `impl<C: Rejection, D: From<C>, L> From<C> for Fail<D, L>` — a bare
/// rejection is the `W::Rejected = W, W::Lanes = NoLanes` case of this.
impl<W: Classify, D: Lift<W::Rejected, Unmapped = Infallible>, M: LaneProfile> From<W>
    for Fail<D, M>
where
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn from(w: W) -> Self {
        match w.classify() {
            Fail::Rejected(r) => match D::lift(r) {
                Ok(d) => Fail::Rejected(d),
                Err(never) => match never {},
            },
            Fail::Denied(d) => Fail::Denied(d.into()),
            Fail::Transient(t) => Fail::Transient(t.into()),
            Fail::Fatal(x) => Fail::Fatal(x.into()),
        }
    }
}

/// `?` into a [`Fault`]: only for a wrapper that never rejects. A rejection or
/// a mixed wrapper entering a `Fault` function stays an explicit narrowing
/// (`.map_err(Fail::narrow_rejected)`), never a silent `?`.
impl<W: Classify<Rejected = Infallible>, M: LaneProfile> From<W> for Fault<M>
where
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn from(w: W) -> Self {
        match w.classify() {
            Fail::Rejected(never) => match never {},
            Fail::Denied(d) => Fault::Denied(d.into()),
            Fail::Transient(t) => Fault::Transient(t.into()),
            Fail::Fatal(x) => Fault::Fatal(x.into()),
        }
    }
}

/// A wrapper that only ever ends in the fatal lane converts into that bare
/// payload directly, so `-> Result<T, Fatal>` is a legal, narrowest
/// signature for a fault-only wrapper.
impl<W: Classify<Rejected = Infallible, Lanes = Profile<false, false, true>>> From<W> for Fatal {
    fn from(w: W) -> Self {
        match w.classify() {
            Fail::Rejected(never) => match never {},
            Fail::Denied(never) => match never {},
            Fail::Transient(never) => match never {},
            Fail::Fatal(x) => x,
        }
    }
}

/// As above, for a wrapper whose only lane is transient.
impl<W: Classify<Rejected = Infallible, Lanes = Profile<false, true, false>>> From<W>
    for Transient
{
    fn from(w: W) -> Self {
        match w.classify() {
            Fail::Rejected(never) => match never {},
            Fail::Denied(never) => match never {},
            Fail::Transient(t) => t,
            Fail::Fatal(never) => match never {},
        }
    }
}

/// The partial-absorption form for any [`Classify`] source — a bare
/// rejection, a fault wrapper, or a mixed wrapper alike. `?` is the total
/// form (the `From` impls above); `.widen()?` is this one, for when the
/// destination's rejection only partially lifts the wrapper's rejected part.
impl<T, C: Classify, P: Lift<C::Rejected>, M: LaneProfile> crate::fail::WidenResult<T, Fail<P, M>>
    for Result<T, C>
where
    <C::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <C::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <C::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
    P::Unmapped: UnmappedInto<M::Fatal>,
{
    fn widen(self) -> Result<T, Fail<P, M>> {
        self.map_err(|c| match c.classify() {
            Fail::Rejected(r) => match P::lift(r) {
                Ok(p) => Fail::Rejected(p),
                Err(u) => Fail::Fatal(u.unmapped_into()),
            },
            Fail::Denied(d) => Fail::Denied(d.into()),
            Fail::Transient(t) => Fail::Transient(t.into()),
            Fail::Fatal(x) => Fail::Fatal(x.into()),
        })
    }
}

/// `.classify::<W>()` — the verb that turns a foreign error into a local
/// [`Classify`] wrapper at a one-off call site, so a function that does not
/// itself return `W` can still enter the lanes through it:
/// `conn.query(..).await.classify::<DbWrite>()?`.
pub trait ClassifyResult<T, E> {
    fn classify<W: Classify + From<E>>(self) -> Result<T, W>;
}

impl<T, E> ClassifyResult<T, E> for Result<T, E> {
    fn classify<W: Classify + From<E>>(self) -> Result<T, W> {
        self.map_err(W::from)
    }
}
