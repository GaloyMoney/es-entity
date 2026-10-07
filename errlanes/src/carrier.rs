//! Carriers: crate-local enums that stand in for [`Fault<L>`] / [`Fail<R, L>`].
//!
//! A carrier is declared with `#[derive(errlanes::Carrier)]` on a hand-written
//! lane enum. It is a distinct
//! nominal type, so two of them can sit in one enum behind separate `#[from]`s,
//! but it converts, widens, narrows, records and retries the way its built-in
//! does. Everything in this module except [`Carrier`] and [`LaneRef`] is hidden
//! plumbing for that derive and for errlanes' own blanket impls.
//!
//! The coherence argument for the tag design (`Kind` for consumers, `Shape` for
//! errlanes itself) is in the `design-options-errlanes-carrier-newtypes` note;
//! read its §2 before touching any blanket impl in this file.

use std::{
    convert::Infallible,
    error::Error,
    fmt::{self, Debug},
};

use crate::{
    classify::Classify,
    fail::{Fail, Fault, Level, Lift, Rejection, UnmappedInto},
    lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient},
    profile::{LaneProfile, NoLanes, Profile},
};

/// Type-level tags carried by [`IntoLanes`]. Hidden: only the macros and
/// errlanes' own blankets name them.
#[doc(hidden)]
pub mod kind {
    /// `Kind`: what a *consumer's* single inbound blanket keys on, by equality
    /// on one value. Everything that is not a carrier is `Plain`.
    pub struct Plain;
    /// `Kind` of a carrier.
    pub struct Carrier;

    /// `Shape`: what errlanes' own blankets key on, through the sealed marker
    /// traits below. Anything [`Classify`](crate::Classify).
    pub struct Source;
    /// `Transient`, `Fatal`, `Denied`, `Exhausted`, `Infallible`.
    pub struct Payload;
    /// [`Fault<L>`](crate::Fault).
    pub struct Fault;
    /// [`Fail<D, L>`](crate::Fail).
    pub struct Fail;
    /// A carrier.
    pub struct CarrierShape;

    /// Shapes `?` may carry into a `Fault<M>`.
    pub trait IntoFault {}
    impl IntoFault for Source {}
    impl IntoFault for Payload {}
    impl IntoFault for CarrierShape {}

    /// Shapes `?` may carry into a `Fail<D, M>`.
    pub trait IntoFail {}
    impl IntoFail for Source {}
    impl IntoFail for Payload {}
    impl IntoFail for Fault {}
    impl IntoFail for CarrierShape {}

    /// Shapes `?` may carry into the bare `Fatal` / `Transient` payloads.
    pub trait IntoBareLane {}
    impl IntoBareLane for Source {}
    impl IntoBareLane for CarrierShape {}
}

/// Every value that can enter the lanes. Hidden: written by errlanes and by
/// the `Carrier` derive, never by hand.
///
/// [`Classify`] itself is unchanged; this is the one trait errlanes' blankets
/// and the carrier derive key on.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not enter the lanes",
    note = "implement `errlanes::Classify` (or derive `Rejection` / `Classify`), or declare \
            a carrier with `#[derive(errlanes::Carrier)]` on a lane enum"
)]
pub trait IntoLanes: Sized {
    type Kind;
    type Shape;
    type Rejected;
    type Lanes: LaneProfile;
    fn into_lanes(self) -> Fail<Self::Rejected, Self::Lanes>;
}

impl<W: Classify> IntoLanes for W {
    type Kind = kind::Plain;
    type Shape = kind::Source;
    type Rejected = W::Rejected;
    type Lanes = W::Lanes;
    fn into_lanes(self) -> Fail<Self::Rejected, Self::Lanes> {
        self.classify()
    }
}

impl<L: LaneProfile> IntoLanes for Fault<L> {
    type Kind = kind::Plain;
    type Shape = kind::Fault;
    type Rejected = Infallible;
    type Lanes = L;
    fn into_lanes(self) -> Fail<Infallible, L> {
        match self {
            Fault::Denied(d) => Fail::Denied(d),
            Fault::Transient(t) => Fail::Transient(t),
            Fault::Fatal(f) => Fail::Fatal(f),
        }
    }
}

impl<D, L: LaneProfile> IntoLanes for Fail<D, L> {
    type Kind = kind::Plain;
    type Shape = kind::Fail;
    type Rejected = D;
    type Lanes = L;
    fn into_lanes(self) -> Fail<D, L> {
        self
    }
}

macro_rules! payload {
    ($ty:ty, $d:literal, $t:literal, $f:literal, $variant:ident) => {
        impl IntoLanes for $ty {
            type Kind = kind::Plain;
            type Shape = kind::Payload;
            type Rejected = Infallible;
            type Lanes = Profile<$d, $t, $f>;
            fn into_lanes(self) -> Fail<Infallible, Self::Lanes> {
                Fail::$variant(self)
            }
        }
    };
}
payload!(Transient, false, true, false, Transient);
payload!(Fatal, false, false, true, Fatal);
payload!(Denied, true, false, false, Denied);

impl IntoLanes for Exhausted {
    type Kind = kind::Plain;
    type Shape = kind::Payload;
    type Rejected = Infallible;
    type Lanes = Profile<false, false, true>;
    fn into_lanes(self) -> Fail<Infallible, Self::Lanes> {
        Fail::Fatal(Fatal::from_error(FatalKind::Exhausted, self))
    }
}

impl IntoLanes for Infallible {
    type Kind = kind::Plain;
    type Shape = kind::Payload;
    type Rejected = Infallible;
    type Lanes = NoLanes;
    fn into_lanes(self) -> Fail<Infallible, NoLanes> {
        match self {}
    }
}

/// The built-in a lane source lands in when nothing names a destination:
/// `Fault<L>` for a source that never rejects, `Fail<R, L>` for one that can.
/// Keyed on the rejected slot, which is `Infallible` or a [`Rejection`] for
/// every source errlanes knows. Hidden: it is the engine of
/// [`ResultExt::widen_via_builtin`](crate::ResultExt::widen_via_builtin).
#[doc(hidden)]
pub trait BuiltinFor<L: LaneProfile>: Sized {
    type Builtin: Error + Send + Sync + 'static;
    fn builtin(f: Fail<Self, L>) -> Self::Builtin;
}

impl<L: LaneProfile> BuiltinFor<L> for Infallible {
    type Builtin = Fault<L>;
    fn builtin(f: Fail<Infallible, L>) -> Fault<L> {
        fail_into_fault(f)
    }
}

impl<R: Rejection, L: LaneProfile> BuiltinFor<L> for R {
    type Builtin = Fail<R, L>;
    fn builtin(f: Fail<R, L>) -> Fail<R, L> {
        f
    }
}

/// A fault-only value, as a `Fault<M>`. The shared body of every `?` into a
/// `Fault`; the lane-subset bounds are what make it total.
fn fail_into_fault<L: LaneProfile, M: LaneProfile>(f: Fail<Infallible, L>) -> Fault<M>
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
{
    match f {
        Fail::Rejected(never) => match never {},
        Fail::Denied(d) => Fault::Denied(d.into()),
        Fail::Transient(t) => Fault::Transient(t.into()),
        Fail::Fatal(x) => Fault::Fatal(x.into()),
    }
}

/// `?` into a [`Fault`]: only for a source that never rejects, and only into a
/// profile that holds all of the source's lanes. A rejection, or a mixed
/// wrapper, entering a `Fault` function stays an explicit narrowing
/// (`narrow_rejected`), never a silent `?`.
///
/// Why this is coherent with the reflexive `From<T> for T`: `Fault<M>`'s own
/// `Shape` is `kind::Fault`, which is not [`kind::IntoFault`], and every type
/// and marker involved is local to errlanes. Do not replace the marker bound
/// with associated-type *equality* across several impls on a generic `W`: two
/// such impls overlap (rust-lang/rust#20400).
impl<W: IntoLanes<Rejected = Infallible>, M: LaneProfile> From<W> for Fault<M>
where
    W::Shape: kind::IntoFault,
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn from(w: W) -> Self {
        fail_into_fault(w.into_lanes())
    }
}

/// `?` into a [`Fail`]: the rejected part must be absorbed totally by `D` (no
/// decision left for the call site to make with `.widen()`); the source's
/// lanes must fit the destination's profile.
impl<W: IntoLanes, D: Lift<W::Rejected, Unmapped = Infallible>, M: LaneProfile> From<W>
    for Fail<D, M>
where
    W::Shape: kind::IntoFail,
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn from(w: W) -> Self {
        fail_absorb(w.into_lanes())
    }
}

fn fail_absorb<R, L: LaneProfile, D: Lift<R, Unmapped = Infallible>, M: LaneProfile>(
    f: Fail<R, L>,
) -> Fail<D, M>
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
{
    match f {
        Fail::Rejected(r) => match D::lift(r) {
            Ok(d) => Fail::Rejected(d),
            Err(never) => match never {},
        },
        Fail::Denied(d) => Fail::Denied(d.into()),
        Fail::Transient(t) => Fail::Transient(t.into()),
        Fail::Fatal(x) => Fail::Fatal(x.into()),
    }
}

/// A source that only ever ends in the fatal lane converts into that bare
/// payload directly, so `-> Result<T, Fatal>` is a legal, narrowest signature
/// for a fault-only wrapper (or a single-lane carrier).
impl<W: IntoLanes<Rejected = Infallible, Lanes = Profile<false, false, true>>> From<W> for Fatal
where
    W::Shape: kind::IntoBareLane,
{
    fn from(w: W) -> Self {
        match w.into_lanes() {
            Fail::Rejected(never) => match never {},
            Fail::Denied(never) => match never {},
            Fail::Transient(never) => match never {},
            Fail::Fatal(x) => x,
        }
    }
}

/// As above, for a source whose only lane is transient.
impl<W: IntoLanes<Rejected = Infallible, Lanes = Profile<false, true, false>>> From<W> for Transient
where
    W::Shape: kind::IntoBareLane,
{
    fn from(w: W) -> Self {
        match w.into_lanes() {
            Fail::Rejected(never) => match never {},
            Fail::Denied(never) => match never {},
            Fail::Transient(t) => t,
            Fail::Fatal(never) => match never {},
        }
    }
}

/// The consumer-side inbound engine: total absorption of `W` into a built-in.
/// A carrier's single blanket `From` is `Repr: Absorb<W>` plus
/// `W: IntoLanes<Kind = Plain>`.
///
/// Total only, the same rule as `?`: a rejection needs a total `Lift`, and a
/// lane the target does not declare must be narrowed first.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{W}` cannot be absorbed into this carrier by `?`",
    note = "a rejection needs a total `Lift`; a lane the carrier does not declare must be \
            narrowed first; another carrier must be listed in `from(..)`"
)]
pub trait Absorb<W> {
    fn absorb(w: W) -> Self;
}

impl<W: IntoLanes<Rejected = Infallible>, M: LaneProfile> Absorb<W> for Fault<M>
where
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn absorb(w: W) -> Self {
        fail_into_fault(w.into_lanes())
    }
}

impl<W: IntoLanes, P: Lift<W::Rejected, Unmapped = Infallible>, M: LaneProfile> Absorb<W>
    for Fail<P, M>
where
    <W::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <W::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <W::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn absorb(w: W) -> Self {
        fail_absorb(w.into_lanes())
    }
}

/// A borrowed view of a carrier's current lane. `Fault::lanes`, `Fail::lanes`
/// and `Carrier::lanes` all return one, so `record`, `message` and `lane` have
/// a single implementation over it.
///
/// `R` is the rejection type; it is [`Infallible`] for a fault-only value,
/// which makes the `Rejected` arm unreachable.
pub enum LaneRef<'a, R> {
    Rejected(&'a R),
    Denied(&'a Denied),
    Transient(&'a Transient),
    Fatal(&'a Fatal),
}

impl<R> Clone for LaneRef<'_, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for LaneRef<'_, R> {}

impl<R: Debug> Debug for LaneRef<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaneRef::Rejected(r) => f.debug_tuple("Rejected").field(r).finish(),
            LaneRef::Denied(d) => f.debug_tuple("Denied").field(d).finish(),
            LaneRef::Transient(t) => f.debug_tuple("Transient").field(t).finish(),
            LaneRef::Fatal(x) => f.debug_tuple("Fatal").field(x).finish(),
        }
    }
}

impl<R> LaneRef<'_, R> {
    /// Which lane this is.
    pub fn lane(&self) -> Lane {
        match self {
            LaneRef::Rejected(_) => Lane::Rejected,
            LaneRef::Denied(_) => Lane::Denied,
            LaneRef::Transient(_) => Lane::Transient,
            LaneRef::Fatal(_) => Lane::Fatal,
        }
    }
}

impl<R: LaneRejected> LaneRef<'_, R> {
    /// The operator-safe one-line text for this failure: exactly what `record`
    /// writes to `exception.message`. `Transient` / `Fatal`: the whole
    /// `source()` chain joined with `": "`. `Denied`: its `Display`.
    /// `Rejected`: the rejection's `code`, never its message (display
    /// discipline — a rejection's message may embed caller-supplied input).
    pub fn message(&self) -> String {
        match self {
            LaneRef::Rejected(d) => d.code_string(),
            LaneRef::Denied(d) => d.to_string(),
            LaneRef::Transient(t) => crate::dynamic::message_chain(*t),
            LaneRef::Fatal(x) => crate::dynamic::message_chain(*x),
        }
    }

    /// The operator level of this failure: the lane default
    /// ([`Lane::level`]), except a `Rejected` outcome reports what its
    /// rejection declares through [`Rejection::level`].
    pub fn level(&self) -> Level {
        match self {
            LaneRef::Rejected(d) => d.level(),
            other => other.lane().level(),
        }
    }

    /// Records this lane onto `span`. See [`crate::FIELDS`].
    #[cfg(feature = "tracing")]
    pub(crate) fn record(&self, span: &tracing::Span) {
        crate::record::record_lanes(span, *self);
    }

    /// Emits this lane as an event at its own level. See [`crate::emit`].
    #[cfg(feature = "tracing")]
    pub(crate) fn emit(&self) {
        crate::record::emit_lanes(*self);
    }
}

/// What the `Rejected` arm of a [`LaneRef`] needs from its `R`: a code, and a
/// level. A [`Rejection`], or `Infallible` (no arm). Hidden: it exists so that
/// one `message` / `record` body serves `Fault` and `Fail` alike.
#[doc(hidden)]
pub trait LaneRejected {
    fn code_string(&self) -> String;
    fn code_str(&self) -> &'static str;
    fn level(&self) -> Level;
}

impl<R: Rejection> LaneRejected for R {
    fn code_string(&self) -> String {
        self.code().to_string()
    }
    fn code_str(&self) -> &'static str {
        Into::<&'static str>::into(self.code())
    }
    fn level(&self) -> Level {
        Rejection::level(self)
    }
}

impl LaneRejected for Infallible {
    fn code_string(&self) -> String {
        match *self {}
    }
    fn code_str(&self) -> &'static str {
        match *self {}
    }
    fn level(&self) -> Level {
        match *self {}
    }
}

impl<L: LaneProfile> Fault<L> {
    /// A borrowed view of the current lane.
    pub fn lanes(&self) -> LaneRef<'_, Infallible> {
        use crate::profile::Slot;
        match self {
            Fault::Denied(d) => LaneRef::Denied(d.marker()),
            Fault::Transient(t) => LaneRef::Transient(t.marker()),
            Fault::Fatal(x) => LaneRef::Fatal(x.marker()),
        }
    }
}

impl<D, L: LaneProfile> Fail<D, L> {
    /// A borrowed view of the current lane.
    pub fn lanes(&self) -> LaneRef<'_, D> {
        use crate::profile::Slot;
        match self {
            Fail::Rejected(d) => LaneRef::Rejected(d),
            Fail::Denied(d) => LaneRef::Denied(d.marker()),
            Fail::Transient(t) => LaneRef::Transient(t.marker()),
            Fail::Fatal(x) => LaneRef::Fatal(x.marker()),
        }
    }
}

mod repr_sealed {
    use super::{Fail, Fault, LaneProfile};
    pub trait Sealed {}
    impl<L: LaneProfile> Sealed for Fault<L> {}
    impl<D, L: LaneProfile> Sealed for Fail<D, L> {}
}

/// The built-ins a [`Carrier`] converts to and from: exactly [`Fault<L>`] and
/// [`Fail<R, L>`]. Sealed.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not `Fault<L>` or `Fail<R, L>`",
    note = "a carrier's `Repr` is the built-in it stands in for"
)]
pub trait Repr: repr_sealed::Sealed + IntoLanes<Kind = kind::Plain> {}
impl<L: LaneProfile> Repr for Fault<L> {}
impl<D, L: LaneProfile> Repr for Fail<D, L> {}

/// A crate-local enum that stands in for `Fault<L>` / `Fail<R, L>`.
/// Implemented by `#[derive(errlanes::Carrier)]` on a lane enum; do not
/// implement it by hand.
///
/// Generic machinery (retry, `record`, `#[errlanes::instrument]`) is written
/// against [`Laned`](crate::Laned), which every carrier whose profile is
/// retryable implements. This trait is for code that needs to move a carrier
/// to or from its built-in.
///
/// ```
/// use errlanes::{Carrier, Fault, lanes};
///
/// #[derive(Debug, errlanes::Carrier)]
/// pub enum HostFault {
///     Transient(errlanes::Transient),
///     Fatal(errlanes::Fatal),
/// }
///
/// fn to_builtin<C: Carrier>(c: C) -> C::Repr {
///     c.into_repr()
/// }
///
/// let fault: Fault<lanes!(Transient, Fatal)> =
///     to_builtin(HostFault::Fatal(errlanes::Fatal::invariant("boom")));
/// assert!(fault.is_fatal());
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a carrier",
    note = "declare one with `#[derive(errlanes::Carrier)]` on a lane enum"
)]
pub trait Carrier:
    Error + Send + Sync + Sized + 'static + IntoLanes<Kind = kind::Carrier, Shape = kind::CarrierShape>
{
    /// The built-in this carrier converts to and from, by value.
    type Repr: Repr<Rejected = <Self as IntoLanes>::Rejected, Lanes = <Self as IntoLanes>::Lanes>;

    fn from_repr(r: Self::Repr) -> Self;
    fn into_repr(self) -> Self::Repr;

    /// A borrowed view of the current lane.
    fn lanes(&self) -> LaneRef<'_, <Self as IntoLanes>::Rejected>;
}

/// Carrier → carrier has no `?` (the reflexive `From<T> for T` makes it
/// impossible), but a carrier widens exactly as its `Repr` does.
#[doc(hidden)]
pub trait WidenBy<S, E2> {
    fn widen_err(s: S) -> E2;
}

impl<L: LaneProfile, M: LaneProfile> WidenBy<Fault<L>, Fault<M>> for kind::Fault
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
{
    fn widen_err(s: Fault<L>) -> Fault<M> {
        s.widen()
    }
}

/// One rule for every rejection remapping. `P: Lift<R>` is satisfied by a total
/// `From<R>` (through errlanes' blanket, `Unmapped = Infallible`) and by a
/// partial `#[lift(Source, unhandled = fatal)]` mapping (`Unmapped = Source`).
/// The `UnmappedInto` bound then enforces, per destination, exactly what each
/// mode needs: a total mapping works into any profile, while a partial one
/// requires the destination to admit `Fatal`.
impl<R, P: Lift<R>, L: LaneProfile, M: LaneProfile> WidenBy<Fail<R, L>, Fail<P, M>> for kind::Fail
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
    P::Unmapped: UnmappedInto<M::Fatal>,
{
    fn widen_err(s: Fail<R, L>) -> Fail<P, M> {
        s.widen()
    }
}

/// The partial-absorption form for any [`Classify`] source — a bare rejection,
/// a fault wrapper, or a mixed wrapper alike. `?` is the total form (the
/// `From` impls above); `.widen()?` is this one, for when the destination's
/// rejection only partially lifts the wrapper's rejected part.
impl<C: IntoLanes, P: Lift<C::Rejected>, M: LaneProfile> WidenBy<C, Fail<P, M>> for kind::Source
where
    <C::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <C::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <C::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
    P::Unmapped: UnmappedInto<M::Fatal>,
{
    fn widen_err(s: C) -> Fail<P, M> {
        s.into_lanes().widen()
    }
}

/// The same act for a destination with no rejected lane: a wrapper that never
/// rejects widens straight into a [`Fault<M>`]. `?` already covers the case
/// where the destination is the function's own return type; this is for the
/// call site that must name the destination because nothing else will infer
/// it — above all a `Box<dyn Error>` boundary, where `?` alone would box the
/// wrapper *unlaned* and the receiving [`Fault::classify`] would then walk
/// past it to whatever foreign error it wraps, reverting the very
/// classification the wrapper exists to override.
///
/// ```
/// use errlanes::{Fault, ResultExt, lanes};
///
/// #[derive(Debug, errlanes::Classify)]
/// #[classify(fatal(CorruptState))]
/// #[error("could not decode stored state")]
/// struct Stored(#[source] std::io::Error);
///
/// fn decode() -> Result<u8, Stored> {
///     Err(Stored(std::io::Error::other("bad bytes")))
/// }
///
/// // A boxed boundary: the destination carrier is named, then boxed.
/// fn boundary() -> Result<u8, Box<dyn std::error::Error + Send + Sync>> {
///     Ok(decode().widen::<Fault<lanes!(Transient, Fatal)>>()?)
/// }
///
/// let fault = errlanes::Fault::classify(&*boundary().unwrap_err());
/// assert!(matches!(fault, Fault::Fatal(f) if f.kind == errlanes::FatalKind::CorruptState));
/// ```
impl<C: IntoLanes<Rejected = Infallible>, M: LaneProfile> WidenBy<C, Fault<M>> for kind::Source
where
    <C::Lanes as LaneProfile>::Denied: Into<M::Denied>,
    <C::Lanes as LaneProfile>::Transient: Into<M::Transient>,
    <C::Lanes as LaneProfile>::Fatal: Into<M::Fatal>,
{
    fn widen_err(s: C) -> Fault<M> {
        fail_into_fault(s.into_lanes())
    }
}

impl<C: Carrier, E2> WidenBy<C, E2> for kind::CarrierShape
where
    <C::Repr as IntoLanes>::Shape: WidenBy<C::Repr, E2>,
{
    fn widen_err(s: C) -> E2 {
        <<C::Repr as IntoLanes>::Shape as WidenBy<C::Repr, E2>>::widen_err(s.into_repr())
    }
}
