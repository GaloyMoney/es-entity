use std::{error::Error, fmt};

use crate::profile::{AllLanes, LaneProfile, NarrowDenied, NarrowTransient};

use crate::lane::{Denied, Exhausted, Fatal, Lane, Transient};

/// Operator level, independent of any tracing dependency in the core API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[cfg(feature = "tracing")]
impl From<Level> for tracing::Level {
    fn from(level: Level) -> Self {
        match level {
            Level::Trace => tracing::Level::TRACE,
            Level::Debug => tracing::Level::DEBUG,
            Level::Info => tracing::Level::INFO,
            Level::Warn => tracing::Level::WARN,
            Level::Error => tracing::Level::ERROR,
        }
    }
}

/// A pure, caller-correctable domain outcome. Implemented by hand or via
/// `#[derive(errlanes::Rejection)]` on a `thiserror` enum.
pub trait Rejection: Error + Send + Sync + 'static {
    /// A stable, typed, wire-safe identity for this outcome.
    type Code: Copy
        + Eq
        + std::hash::Hash
        + fmt::Debug
        + fmt::Display
        + Into<&'static str>
        + Send
        + Sync
        + 'static;

    fn code(&self) -> Self::Code;

    /// Operator level for this outcome. Rejections default to `Info`.
    fn level(&self) -> Level {
        Level::Info
    }
}

/// Consuming mapping. `#[derive(Lift)]` with `#[lift(Source)]` generates an exhaustive
/// mapping with `Unmapped = Infallible` and a total `From<Source>` conversion.
/// `#[lift(Source, unhandled = fatal)]` returns the original unmapped source;
/// [`Fail::lift`] wraps it as a fatal invariant with its source intact.
/// The derive does not require or implement [`Rejection`]. Derive `Rejection`
/// separately to provide codes and levels; simple lift mappings forward that
/// metadata by default unless the destination declares its own.
pub trait Lift<X>: Sized {
    type Unmapped;
    fn lift(x: X) -> Result<Self, Self::Unmapped>;
}

/// A total `From` counts as a strict lift, so one call-site method can require
/// only `Lift` and still cover both mapping modes.
impl<X, P: From<X>> Lift<X> for P {
    type Unmapped = core::convert::Infallible;
    fn lift(x: X) -> Result<Self, Self::Unmapped> {
        Ok(P::from(x))
    }
}

/// Conversion of an unmapped value into an enabled fatal slot. Strict lifts
/// have no unmapped values and therefore work with any destination profile.
#[doc(hidden)]
pub trait UnmappedInto<F> {
    fn unmapped_into(self) -> F;
}
impl<F> UnmappedInto<F> for core::convert::Infallible {
    fn unmapped_into(self) -> F {
        match self {}
    }
}
impl<R: Rejection> UnmappedInto<Fatal> for R {
    fn unmapped_into(self) -> Fatal {
        Fatal::from_error(crate::FatalKind::Invariant, self)
            .with_context("unhandled rejection at partial lift")
    }
}

/// `Fail` minus the `Rejected` lane: what an operation that cannot reject
/// (nothing about it is the caller's to correct) returns. Reads return
/// `Fault<L>`; writes return `Fail<{Entity}ConstraintViolation, L>` — the type
/// itself says whether a call can ever hand back a domain outcome.
#[derive(Debug, Clone)]
pub enum Fault<L: LaneProfile = AllLanes> {
    Denied(L::Denied),
    Transient(L::Transient),
    Fatal(L::Fatal),
}

impl<L: LaneProfile> Fault<L> {
    pub fn lane(&self) -> Lane {
        match self {
            Fault::Denied(_) => Lane::Denied,
            Fault::Transient(_) => Lane::Transient,
            Fault::Fatal(_) => Lane::Fatal,
        }
    }

    pub fn is_transient(&self) -> bool {
        matches!(self, Fault::Transient(_))
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Fault::Fatal(_))
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Fault::Denied(_))
    }

    /// The operator-safe one-line text for this failure — exactly what
    /// `record` writes to `exception.message`, for a boundary that must
    /// persist it rather than (or as well as) record it. `Transient`/
    /// `Fatal`: the whole `source()` chain joined with `": "`. `Denied`: its
    /// `Display`.
    pub fn message(&self) -> String {
        match self {
            Fault::Denied(d) => d.to_string(),
            Fault::Transient(t) => crate::dynamic::message_chain(t),
            Fault::Fatal(x) => crate::dynamic::message_chain(x),
        }
    }

    /// Consumes the `Transient` lane, yielding the same profile with its
    /// transient slot disabled. `attempts` is what the retry loop counted; an
    /// exhausted transient becomes `Fatal(Exhausted)` with the last transient
    /// as its source.
    pub fn narrow_transient(self, attempts: u32) -> Fault<crate::profile::WithoutTransient<L>>
    where
        L::Transient: NarrowTransient<L::Fatal>,
    {
        match self {
            Fault::Denied(d) => Fault::Denied(d),
            Fault::Transient(last) => Fault::Fatal(last.narrow(attempts)),
            Fault::Fatal(f) => Fault::Fatal(f),
        }
    }

    /// Narrows away the `Denied` lane: a denial at a boundary with no
    /// subject (code running as the system) becomes `Fatal(Denied)` with
    /// the `Denied` as its source.
    pub fn narrow_denied(self) -> Fault<crate::profile::WithoutDenied<L>>
    where
        L::Denied: NarrowDenied<L::Fatal>,
    {
        match self {
            Fault::Denied(d) => Fault::Fatal(d.narrow()),
            Fault::Transient(t) => Fault::Transient(t),
            Fault::Fatal(f) => Fault::Fatal(f),
        }
    }
}

/// Borrowed lane accessors. `min_exhaustive_patterns` lets a *by-value* match
/// name only the lanes a profile enables, but a borrowed match still demands an
/// arm for every variant. These accessors are the borrowed form, so no caller
/// ever has to write a `match *never {}` arm. Each is available only when the
/// profile enables that lane, so `e.as_denied()` on a no-denial profile is a
/// compile error rather than a permanent `None`.
impl<L: LaneProfile<Denied = Denied>> Fault<L> {
    pub fn as_denied(&self) -> Option<&Denied> {
        match self {
            Fault::Denied(d) => Some(d),
            _ => None,
        }
    }
}

impl<L: LaneProfile<Transient = Transient>> Fault<L> {
    pub fn as_transient(&self) -> Option<&Transient> {
        match self {
            Fault::Transient(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_congestion(&self) -> bool {
        self.as_transient().is_some_and(Transient::is_congestion)
    }
}

impl<L: LaneProfile<Fatal = Fatal>> Fault<L> {
    pub fn as_fatal(&self) -> Option<&Fatal> {
        match self {
            Fault::Fatal(f) => Some(f),
            _ => None,
        }
    }
}

impl<D, L: LaneProfile<Denied = Denied>> Fail<D, L> {
    pub fn as_denied(&self) -> Option<&Denied> {
        match self {
            Fail::Denied(d) => Some(d),
            _ => None,
        }
    }
}

impl<D, L: LaneProfile<Transient = Transient>> Fail<D, L> {
    pub fn as_transient(&self) -> Option<&Transient> {
        match self {
            Fail::Transient(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_congestion(&self) -> bool {
        self.as_transient().is_some_and(Transient::is_congestion)
    }
}

impl<D, L: LaneProfile<Fatal = Fatal>> Fail<D, L> {
    pub fn as_fatal(&self) -> Option<&Fatal> {
        match self {
            Fail::Fatal(f) => Some(f),
            _ => None,
        }
    }
}

impl<L: LaneProfile> fmt::Display for Fault<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Denied(d) => write!(f, "{d}"),
            Fault::Transient(t) => write!(f, "{t}"),
            Fault::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<L: LaneProfile> Error for Fault<L> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // Same contract as `Fail::source` (pitfall 2): the lane payload
        // itself, so `lane_of` can downcast it.
        match self {
            Fault::Denied(d) => Some(d),
            Fault::Transient(t) => Some(t),
            Fault::Fatal(x) => Some(x),
        }
    }
}

impl<L: LaneProfile> From<Transient> for Fault<L>
where
    L: LaneProfile<Transient = Transient>,
{
    fn from(t: Transient) -> Self {
        Fault::Transient(t)
    }
}

impl<L: LaneProfile> From<Fatal> for Fault<L>
where
    L: LaneProfile<Fatal = Fatal>,
{
    fn from(f: Fatal) -> Self {
        Fault::Fatal(f)
    }
}

impl<L: LaneProfile> From<Denied> for Fault<L>
where
    L: LaneProfile<Denied = Denied>,
{
    fn from(d: Denied) -> Self {
        Fault::Denied(d)
    }
}

impl<L: LaneProfile> From<Exhausted> for Fault<L>
where
    L: LaneProfile<Fatal = Fatal>,
{
    fn from(e: Exhausted) -> Self {
        Fault::Fatal(Fatal::from_error(crate::lane::FatalKind::Exhausted, e))
    }
}

/// `Infallible` is what a disabled lane slot is, so a value proven never to
/// exist converts trivially — `match e {}`.
impl<L: LaneProfile> From<core::convert::Infallible> for Fault<L> {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

/// The generic view over a domain rejection `D`, before retries have run.
///
/// **Display discipline**: [`Display`](fmt::Display)'s `Rejected` arm keeps
/// its `rejected: {d}` prefix for logs, but no boundary may build a
/// user-facing message from `to_string()` — a rejection's message may embed
/// caller-supplied input. Use [`as_rejected`](Fail::as_rejected) and
/// [`Rejection::code`] instead: `record_fail` keys `error.code` off the code,
/// never the message, and a GraphQL boundary should do the same for its error
/// extension.
#[derive(Debug, Clone)]
pub enum Fail<D, L: LaneProfile = AllLanes> {
    Rejected(D),
    Denied(L::Denied),
    Transient(L::Transient),
    Fatal(L::Fatal),
}

impl<D, L: LaneProfile> Fail<D, L> {
    pub fn lane(&self) -> Lane {
        match self {
            Fail::Rejected(_) => Lane::Rejected,
            Fail::Denied(_) => Lane::Denied,
            Fail::Transient(_) => Lane::Transient,
            Fail::Fatal(_) => Lane::Fatal,
        }
    }

    /// Widen rejection and lane profile without changing any classification.
    pub fn widen<P: From<D>, M: LaneProfile>(self) -> Fail<P, M>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
    {
        match self {
            Self::Rejected(d) => Fail::Rejected(d.into()),
            Self::Denied(d) => Fail::Denied(d.into()),
            Self::Transient(t) => Fail::Transient(t.into()),
            Self::Fatal(f) => Fail::Fatal(f.into()),
        }
    }

    /// Explicit partial mapping. Unmapped values retain their source as invariants.
    pub fn lift<P: Lift<D>, M: LaneProfile>(self) -> Fail<P, M>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
        P::Unmapped: UnmappedInto<M::Fatal>,
    {
        match self {
            Self::Rejected(d) => match P::lift(d) {
                Ok(mapped) => Fail::Rejected(mapped),
                Err(unmapped) => Fail::Fatal(unmapped.unmapped_into()),
            },
            Self::Denied(d) => Fail::Denied(d.into()),
            Self::Transient(t) => Fail::Transient(t.into()),
            Self::Fatal(f) => Fail::Fatal(f.into()),
        }
    }

    pub fn widen_with<P, M: LaneProfile<Fatal = Fatal>>(
        self,
        f: impl FnOnce(D) -> Result<P, Fatal>,
    ) -> Fail<P, M>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
    {
        match self {
            Self::Rejected(d) => match f(d) {
                Ok(p) => Fail::Rejected(p),
                Err(e) => Fail::Fatal(e),
            },
            Self::Denied(d) => Fail::Denied(d.into()),
            Self::Transient(t) => Fail::Transient(t.into()),
            Self::Fatal(f) => Fail::Fatal(f.into()),
        }
    }

    pub fn map_rejected<D2>(self, f: impl FnOnce(D) -> D2) -> Fail<D2, L> {
        match self {
            Fail::Rejected(d) => Fail::Rejected(f(d)),
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(fatal) => Fail::Fatal(fatal),
        }
    }

    /// Narrows to the domain outcome, or the non-domain fault. `let d =
    /// e.rejected()?;` propagates the fault into any enclosing `Fail<_, L>` (or
    /// a `Failure` carrier) via the blanket `From<Fault<L>>`.
    pub fn rejected(self) -> Result<D, Fault<L>> {
        match self {
            Fail::Rejected(d) => Ok(d),
            Fail::Denied(d) => Err(Fault::Denied(d)),
            Fail::Transient(t) => Err(Fault::Transient(t)),
            Fail::Fatal(f) => Err(Fault::Fatal(f)),
        }
    }

    pub fn as_rejected(&self) -> Option<&D> {
        match self {
            Fail::Rejected(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_transient(&self) -> bool {
        matches!(self, Fail::Transient(_))
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Fail::Fatal(_))
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Fail::Denied(_))
    }

    /// The operator-safe one-line text for this failure — exactly what
    /// `record` writes to `exception.message`, for a boundary that must
    /// persist it rather than (or as well as) record it. `Transient`/
    /// `Fatal`: the whole `source()` chain joined with `": "`. `Denied`: its
    /// `Display`. `Rejected`: the rejection's `code`, never its message
    /// (display discipline — a rejection's message may embed caller-supplied
    /// input).
    pub fn message(&self) -> String
    where
        D: Rejection,
    {
        match self {
            Fail::Rejected(d) => d.code().to_string(),
            Fail::Denied(d) => d.to_string(),
            Fail::Transient(t) => crate::dynamic::message_chain(t),
            Fail::Fatal(x) => crate::dynamic::message_chain(x),
        }
    }

    /// Consumes the `Transient` lane, yielding the same rejection over `L`
    /// with its transient slot disabled. An exhausted transient becomes
    /// `Fatal(Exhausted)`, carrying `attempts` and the last transient as its
    /// source.
    pub fn narrow_transient(self, attempts: u32) -> Fail<D, crate::profile::WithoutTransient<L>>
    where
        L::Transient: NarrowTransient<L::Fatal>,
    {
        match self {
            Fail::Rejected(d) => Fail::Rejected(d),
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(last) => Fail::Fatal(last.narrow(attempts)),
            Fail::Fatal(f) => Fail::Fatal(f),
        }
    }

    /// Narrows away the `Denied` lane: a denial at a boundary with no
    /// subject (code running as the system) becomes `Fatal(Denied)` with
    /// the `Denied` as its source.
    pub fn narrow_denied(self) -> Fail<D, crate::profile::WithoutDenied<L>>
    where
        L::Denied: NarrowDenied<L::Fatal>,
    {
        match self {
            Fail::Rejected(d) => Fail::Rejected(d),
            Fail::Denied(d) => Fail::Fatal(d.narrow()),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(f) => Fail::Fatal(f),
        }
    }

    /// Narrows away the `Rejected` lane: a rejection at a boundary with no
    /// caller to correct it is an invariant violation, and becomes
    /// `Fatal(Invariant)` with the rejection as its source — the same rule
    /// a partial `#[lift(.., unhandled = fatal)]` applies. Nothing is left
    /// to reject, so the result is a `Fault`.
    pub fn narrow_rejected(self) -> Fault<L>
    where
        D: Rejection,
        L: LaneProfile<Fatal = Fatal>,
    {
        match self {
            Fail::Rejected(d) => Fault::Fatal(
                Fatal::from_error(crate::FatalKind::Invariant, d)
                    .with_context("rejected with no caller to correct"),
            ),
            Fail::Denied(d) => Fault::Denied(d),
            Fail::Transient(t) => Fault::Transient(t),
            Fail::Fatal(f) => Fault::Fatal(f),
        }
    }
}

/// The real blanket the `Infallible` encoding could not have: `Fault<L>` is not
/// `Fail`, so this does not overlap `From<T> for T`.
impl<D, S: LaneProfile, L: LaneProfile> From<Fault<S>> for Fail<D, L>
where
    S::Denied: Into<L::Denied>,
    S::Transient: Into<L::Transient>,
    S::Fatal: Into<L::Fatal>,
{
    fn from(f: Fault<S>) -> Self {
        match f {
            Fault::Denied(d) => Fail::Denied(d.into()),
            Fault::Transient(t) => Fail::Transient(t.into()),
            Fault::Fatal(x) => Fail::Fatal(x.into()),
        }
    }
}

impl<D, L: LaneProfile> From<Transient> for Fail<D, L>
where
    L: LaneProfile<Transient = Transient>,
{
    fn from(t: Transient) -> Self {
        Fail::Transient(t)
    }
}

impl<D, L: LaneProfile> From<Fatal> for Fail<D, L>
where
    L: LaneProfile<Fatal = Fatal>,
{
    fn from(f: Fatal) -> Self {
        Fail::Fatal(f)
    }
}

impl<D, L: LaneProfile> From<Denied> for Fail<D, L>
where
    L: LaneProfile<Denied = Denied>,
{
    fn from(d: Denied) -> Self {
        Fail::Denied(d)
    }
}

impl<D, L: LaneProfile> From<Exhausted> for Fail<D, L>
where
    L: LaneProfile<Fatal = Fatal>,
{
    fn from(e: Exhausted) -> Self {
        Fail::Fatal(Fatal::from_error(crate::lane::FatalKind::Exhausted, e))
    }
}

impl<C: Rejection, D: From<C>, L: LaneProfile> From<C> for Fail<D, L> {
    fn from(d: C) -> Self {
        Fail::Rejected(d.into())
    }
}

impl<D: fmt::Display, L: LaneProfile> fmt::Display for Fail<D, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Rejected(d) => write!(f, "rejected: {d}"),
            Fail::Denied(d) => write!(f, "{d}"),
            Fail::Transient(t) => write!(f, "{t}"),
            Fail::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<D: Error + 'static, L: LaneProfile> Error for Fail<D, L> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            // The lane payload itself must be the *next* link in the chain
            // so that `lane_of` can find it by downcasting.
            Fail::Rejected(d) => Some(d),
            Fail::Denied(d) => Some(d),
            Fail::Transient(t) => Some(t),
            Fail::Fatal(x) => Some(x),
        }
    }
}

/// Implemented by per-crate carrier newtypes, typically via
/// `#[derive(errlanes::Failure)]`. Generic machinery (retry, boundary
/// recorders) is written against this, not against `Fail<D, L>` directly.
pub trait Failure: Error + Send + Sync + Sized + 'static {
    type Rejection: Rejection;
    type Lanes: LaneProfile;

    fn into_fail(self) -> Fail<Self::Rejection, Self::Lanes>;
    fn from_fail(f: Fail<Self::Rejection, Self::Lanes>) -> Self;
    fn as_fail(&self) -> &Fail<Self::Rejection, Self::Lanes>;

    fn lane(&self) -> Lane {
        self.as_fail().lane()
    }

    fn is_transient(&self) -> bool {
        matches!(self.lane(), Lane::Transient)
    }
}

impl<D: Rejection, L: LaneProfile> Failure for Fail<D, L> {
    type Rejection = D;
    type Lanes = L;

    fn into_fail(self) -> Fail<Self::Rejection, Self::Lanes> {
        self
    }

    fn from_fail(f: Fail<Self::Rejection, Self::Lanes>) -> Self {
        f
    }

    fn as_fail(&self) -> &Fail<Self::Rejection, Self::Lanes> {
        self
    }
}

mod sealed {
    pub trait Sealed {}
    impl<F: super::Failure> Sealed for F {}
    impl<L: super::LaneProfile> Sealed for super::Fault<L> {}
}

/// Sealed. The thing `retry`/`record` need from any error they are handed:
/// its lane, and how to narrow it. Implemented by every [`Failure`] (which
/// covers `Fail<D, L>` itself and every carrier) and by [`Fault<L>`] — the two
/// shapes a generated repo op can return.
pub trait Laned: sealed::Sealed + Error + Send + Sync + 'static + Sized {
    type WithoutTransient: Error + Send + Sync + 'static;

    fn lane(&self) -> Lane;

    fn is_transient(&self) -> bool {
        self.lane() == Lane::Transient
    }

    fn is_fatal(&self) -> bool {
        self.lane() == Lane::Fatal
    }

    fn is_denied(&self) -> bool {
        self.lane() == Lane::Denied
    }

    fn narrow_transient(self, attempts: u32) -> Self::WithoutTransient;

    /// The operator-safe one-line text for this failure — exactly what
    /// `record` writes to `exception.message`, for a boundary that must
    /// persist it rather than (or as well as) record it. `Transient`/
    /// `Fatal`: the whole `source()` chain joined with `": "`. `Denied`: its
    /// `Display`. `Rejected`: the rejection's `code`, never its message
    /// (display discipline — a rejection's message may embed caller-supplied
    /// input).
    fn message(&self) -> String;

    #[cfg(feature = "tracing")]
    fn record(&self, span: &tracing::Span);
}

/// Note the `NarrowTransient` bound: a profile that admits `Transient` but
/// not `Fatal` is not `Laned`, so `retry` cannot be handed one. Retrying an
/// operation that claims it can never fail permanently is exactly the
/// contradiction the bound rules out.
impl<F: Failure> Laned for F
where
    <F::Lanes as LaneProfile>::Transient: NarrowTransient<<F::Lanes as LaneProfile>::Fatal>,
{
    type WithoutTransient = Fail<F::Rejection, crate::profile::WithoutTransient<F::Lanes>>;

    fn lane(&self) -> Lane {
        Failure::lane(self)
    }

    fn narrow_transient(self, attempts: u32) -> Self::WithoutTransient {
        self.into_fail().narrow_transient(attempts)
    }

    fn message(&self) -> String {
        self.as_fail().message()
    }

    #[cfg(feature = "tracing")]
    fn record(&self, span: &tracing::Span) {
        crate::record::record_fail(span, self.as_fail());
    }
}

impl<L: LaneProfile> Laned for Fault<L>
where
    L::Transient: NarrowTransient<L::Fatal>,
{
    type WithoutTransient = Fault<crate::profile::WithoutTransient<L>>;

    fn lane(&self) -> Lane {
        Fault::lane(self)
    }

    fn narrow_transient(self, attempts: u32) -> Self::WithoutTransient {
        Fault::narrow_transient(self, attempts)
    }

    fn message(&self) -> String {
        Fault::message(self)
    }

    #[cfg(feature = "tracing")]
    fn record(&self, span: &tracing::Span) {
        crate::record::record_fault(span, self);
    }
}

/// Target-inferred widening for results containing [`Fault`] or [`Fail`].
///
/// `Fault` results widen to `Fault`; `Fail` results widen to `Fail`, converting
/// the rejection through [`From`]. Success values and fault payloads are preserved.
/// Widening can add lanes, but cannot silently discard an enabled lane:
///
/// ```compile_fail
/// use errlanes::{Fault, WidenResult, lanes};
/// fn discard_denied(value: Result<(), Fault<lanes!(Denied, Fatal)>>)
///     -> Result<(), Fault<lanes!(Fatal)>>
/// {
///     value.widen()
/// }
/// ```
pub trait WidenResult<T, E>: Sized {
    fn widen(self) -> Result<T, E>;
}

impl<T, L: LaneProfile, M: LaneProfile> WidenResult<T, Fault<M>> for Result<T, Fault<L>>
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
{
    fn widen(self) -> Result<T, Fault<M>> {
        self.map_err(Fault::widen)
    }
}

/// One rule for every rejection remapping. `P: Lift<R>` is satisfied by a total
/// `From<R>` (through errlanes' blanket, `Unmapped = Infallible`) and by a
/// partial `#[lift(Source, unhandled = fatal)]` mapping (`Unmapped = Source`).
/// The `UnmappedInto` bound then enforces, per destination, exactly what each
/// mode needs: a total mapping works into any profile, while a partial one
/// requires the destination to admit `Fatal`. The strict/partial choice is
/// declared once on the destination enum, so the call site does not repeat it.
impl<T, R, P: Lift<R>, L: LaneProfile, M: LaneProfile> WidenResult<T, Fail<P, M>>
    for Result<T, Fail<R, L>>
where
    L::Denied: Into<M::Denied>,
    L::Transient: Into<M::Transient>,
    L::Fatal: Into<M::Fatal>,
    P::Unmapped: UnmappedInto<M::Fatal>,
{
    fn widen(self) -> Result<T, Fail<P, M>> {
        self.map_err(Fail::lift)
    }
}

/// A bare rejection is a `Fail` with no fault lanes, so it widens by the same
/// rule. A total mapping already propagates with `?` through `From`; this is
/// what a *partial* mapping needs, since it deliberately has no `From` — the
/// unmapped cases become `Fatal`, so the destination must admit it. `Fail` is
/// never a `Rejection`, so this cannot overlap the carrier impls above.
impl<T, C: Rejection, P: Lift<C>, M: LaneProfile> WidenResult<T, Fail<P, M>> for Result<T, C>
where
    P::Unmapped: UnmappedInto<M::Fatal>,
{
    fn widen(self) -> Result<T, Fail<P, M>> {
        self.map_err(|rejection| match P::lift(rejection) {
            Ok(mapped) => Fail::Rejected(mapped),
            Err(unmapped) => Fail::Fatal(unmapped.unmapped_into()),
        })
    }
}

impl<L: LaneProfile> Fault<L> {
    pub fn widen<M: LaneProfile>(self) -> Fault<M>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
    {
        match self {
            Self::Denied(d) => Fault::Denied(d.into()),
            Self::Transient(t) => Fault::Transient(t.into()),
            Self::Fatal(f) => Fault::Fatal(f.into()),
        }
    }
}

/// Compile-time field metadata used by the rejection composition protocol.
#[doc(hidden)]
pub trait RejectionField<const VARIANT: u64, const FIELD: usize> {
    type Type;
}

/// Borrowed metadata protocol; forwarding does not clone or reconstruct errors.
#[doc(hidden)]
pub trait RejectionMetadata<const VARIANT: u64>: Rejection {
    type Fields<'a>;
    fn field_code(fields: Self::Fields<'_>) -> Self::Code;
    fn field_level(fields: Self::Fields<'_>) -> Level;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::{FatalKind, TransientKind};

    #[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
    struct SmallCode;
    impl fmt::Display for SmallCode {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("SMALL")
        }
    }
    impl From<SmallCode> for &'static str {
        fn from(_: SmallCode) -> Self {
            "SMALL"
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Small;
    impl fmt::Display for Small {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("small")
        }
    }
    impl Error for Small {}
    impl Rejection for Small {
        type Code = SmallCode;
        fn code(&self) -> SmallCode {
            SmallCode
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Big(Small);
    impl fmt::Display for Big {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "big({})", self.0)
        }
    }
    impl Error for Big {}
    impl From<Small> for Big {
        fn from(s: Small) -> Self {
            Big(s)
        }
    }
    impl Rejection for Big {
        type Code = SmallCode;
        fn code(&self) -> SmallCode {
            self.0.code()
        }
    }

    #[test]
    fn widen_preserves_the_lane() {
        let f: Fail<Small> = Fail::Rejected(Small);
        let widened: Fail<Big> = f.widen();
        assert_eq!(widened.lane(), Lane::Rejected);
        assert_eq!(widened.as_rejected(), Some(&Big(Small)));

        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        let widened: Fail<Big> = t.widen();
        assert_eq!(widened.lane(), Lane::Transient);
    }

    #[test]
    fn widen_with_demotes_an_unmapped_rejection_to_fatal() {
        let f: Fail<Small> = Fail::Rejected(Small);
        let widened: Fail<Big> = f.widen_with(|_| Err(Fatal::invariant("unmapped")));
        assert_eq!(widened.lane(), Lane::Fatal);

        let f: Fail<Small> = Fail::Rejected(Small);
        let widened: Fail<Big> = f.widen_with(|s| Ok(Big(s)));
        assert_eq!(widened.lane(), Lane::Rejected);
    }

    #[test]
    fn narrow_transient_turns_transient_into_exhausted_and_leaves_everything_else_alone() {
        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        match t.narrow_transient(3) {
            Fail::Fatal(f) => {
                assert_eq!(f.kind, FatalKind::Exhausted);
                let e = Error::source(&f)
                    .and_then(|s| s.downcast_ref::<Exhausted>())
                    .expect("exhaustion is the fatal's source");
                assert_eq!(e.attempts, 3);
                assert_eq!(e.last.kind, TransientKind::Deadlock);
            }
            other => panic!("expected Fatal(Exhausted), got {other:?}"),
        }

        let r: Fail<Small> = Fail::Rejected(Small);
        assert!(matches!(r.narrow_transient(1), Fail::Rejected(Small)));

        let fatal: Fail<Small> = Fatal::new(FatalKind::Config).into();
        assert!(matches!(fatal.narrow_transient(1), Fail::Fatal(_)));
    }

    #[test]
    fn fault_converts_into_any_fail_via_the_real_blanket() {
        let fault: Fault = Fatal::new(FatalKind::CorruptState).into();
        let widened: Fail<Small> = fault.into();
        assert_eq!(widened.lane(), Lane::Fatal);
    }

    #[test]
    fn rejected_narrows_or_returns_the_fault() {
        let f: Fail<Small> = Fail::Rejected(Small);
        assert_eq!(f.rejected().unwrap(), Small);

        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        assert!(t.rejected().is_err());
    }

    #[test]
    fn fail_source_is_the_lane_payload_itself_so_lane_of_can_downcast_it() {
        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        let source = std::error::Error::source(&t).expect("source present");
        assert!(source.is::<Transient>());
    }

    #[test]
    fn fault_source_is_the_lane_payload_itself_so_lane_of_can_downcast_it() {
        let t: Fault = Transient::new(TransientKind::Deadlock).into();
        let source = std::error::Error::source(&t).expect("source present");
        assert!(source.is::<Transient>());
    }

    #[test]
    fn laned_narrow_transient_agrees_between_fail_and_fault() {
        fn exhausted_attempts(e: &(dyn Error + 'static)) -> u32 {
            e.source()
                .and_then(|s| s.downcast_ref::<Exhausted>())
                .expect("exhaustion is the fatal's source")
                .attempts
        }

        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        let narrowed: Fail<Small, crate::profile::WithoutTransient<AllLanes>> =
            Laned::narrow_transient(t, 2);
        assert_eq!(narrowed.lane(), Lane::Fatal);
        assert_eq!(exhausted_attempts(narrowed.as_fatal().unwrap()), 2);

        let t: Fault = Transient::new(TransientKind::Deadlock).into();
        let narrowed: Fault<crate::profile::WithoutTransient<AllLanes>> =
            Laned::narrow_transient(t, 2);
        assert_eq!(narrowed.lane(), Lane::Fatal);
        assert_eq!(exhausted_attempts(narrowed.as_fatal().unwrap()), 2);
    }
}
