use std::{error::Error, fmt};

use crate::profile::{AllLanes, LaneProfile, TransientSlot};

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

/// Legacy discriminator-based rejection contract. New code maps enum variants
/// with `#[lift(Source)]`; generated repositories do not require this trait.
pub trait Liftable: Rejection {
    type Key: Copy + Eq + fmt::Debug + Into<&'static str>;

    /// `None` means the discriminator is unknown to the caller — always
    /// demoted to [`Fatal`] by [`Lift::lift`].
    fn key(&self) -> Option<Self::Key>;
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
// Compatibility with the v1 discriminator adapter.
impl UnmappedInto<Fatal> for Fatal {
    fn unmapped_into(self) -> Fatal {
        self
    }
}

#[doc(hidden)]
pub trait ExhaustionInto<F> {
    fn exhaustion_into(self) -> F;
}
impl<F> ExhaustionInto<F> for core::convert::Infallible {
    fn exhaustion_into(self) -> F {
        match self {}
    }
}
impl ExhaustionInto<Fatal> for Exhausted {
    fn exhaustion_into(self) -> Fatal {
        Fatal::from_error(crate::FatalKind::Exhausted, self)
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

    /// Consumes the `Transient` lane. `attempts` is what the retry loop
    /// counted.
    pub fn settle(self, attempts: u32) -> SettledFault<L> {
        match self {
            Fault::Denied(d) => SettledFault::Denied(d),
            Fault::Transient(last) => SettledFault::Exhausted(last.settle(attempts)),
            Fault::Fatal(f) => SettledFault::Fatal(f),
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

/// The uninhabited "this hook is not configured" default error type
/// (`post_persist_hook`/`post_hydrate_hook`'s bound is `Fault<L>: From<X>`, and
/// the no-hook case sets `X = core::convert::Infallible`) converts trivially.
impl<L: LaneProfile> From<core::convert::Infallible> for Fault<L> {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

impl<L: LaneProfile> From<Box<dyn Error + Send + Sync>> for Fault<L>
where
    L: LaneProfile<Fatal = Fatal>,
{
    fn from(e: Box<dyn Error + Send + Sync>) -> Self {
        Fault::Fatal(Fatal::from_boxed(crate::lane::FatalKind::Dependency, e))
    }
}

/// `Fault<L>` after retries have run: no `Transient` arm.
#[derive(Debug, Clone)]
pub enum SettledFault<L: LaneProfile = AllLanes> {
    Denied(L::Denied),
    Exhausted(<L::Transient as TransientSlot>::Exhausted),
    Fatal(L::Fatal),
}

impl<L: LaneProfile> SettledFault<L> {
    /// `Exhausted` reports as `Lane::Fatal`, as `Settled::lane` does.
    pub fn lane(&self) -> Lane {
        match self {
            SettledFault::Denied(_) => Lane::Denied,
            SettledFault::Exhausted(_) => Lane::Fatal,
            SettledFault::Fatal(_) => Lane::Fatal,
        }
    }
}

impl<L: LaneProfile> fmt::Display for SettledFault<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettledFault::Denied(d) => write!(f, "{d}"),
            SettledFault::Exhausted(e) => write!(f, "{e}"),
            SettledFault::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<L: LaneProfile> Error for SettledFault<L> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SettledFault::Denied(d) => Some(d),
            SettledFault::Exhausted(e) => Some(e),
            SettledFault::Fatal(x) => Some(x),
        }
    }
}

impl<D, L: LaneProfile> From<SettledFault<L>> for Settled<D, L> {
    fn from(f: SettledFault<L>) -> Self {
        match f {
            SettledFault::Denied(d) => Settled::Denied(d),
            SettledFault::Exhausted(e) => Settled::Exhausted(e),
            SettledFault::Fatal(x) => Settled::Fatal(x),
        }
    }
}

/// The generic view over a domain rejection `D`, before retries have run.
///
/// **Display discipline**: [`Display`](fmt::Display)'s `Rejected` arm keeps
/// its `rejected: {d}` prefix for logs, but no boundary may build a
/// user-facing message from `to_string()` — a rejection's message may embed
/// caller-supplied input. Use [`as_rejected`](Fail::as_rejected) and
/// [`Rejection::code`] instead: `record`/`record_fail` key `error.code` off
/// the code, never the message, and a GraphQL boundary should do the same
/// for its error extension.
#[derive(Debug, Clone)]
pub enum Fail<D, L: LaneProfile = AllLanes> {
    Rejected(D),
    Denied(L::Denied),
    Transient(L::Transient),
    Fatal(L::Fatal),
}

/// The view over a domain rejection `D` after retries have run: no
/// `Transient` arm, so nothing downstream can forget to handle it.
#[derive(Debug, Clone)]
pub enum Settled<D, L: LaneProfile = AllLanes> {
    Rejected(D),
    Denied(L::Denied),
    Exhausted(<L::Transient as TransientSlot>::Exhausted),
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

    /// Consumes the `Transient` lane. `attempts` is what the retry loop
    /// counted.
    pub fn settle(self, attempts: u32) -> Settled<D, L> {
        match self {
            Fail::Rejected(d) => Settled::Rejected(d),
            Fail::Denied(d) => Settled::Denied(d),
            Fail::Transient(last) => Settled::Exhausted(last.settle(attempts)),
            Fail::Fatal(f) => Settled::Fatal(f),
        }
    }
}

impl<D, L: LaneProfile> Settled<D, L> {
    /// `Exhausted` reports as `Lane::Fatal` — by the time retries are done,
    /// it pages just like any other fatal outcome.
    pub fn lane(&self) -> Lane {
        match self {
            Settled::Rejected(_) => Lane::Rejected,
            Settled::Denied(_) => Lane::Denied,
            Settled::Exhausted(_) => Lane::Fatal,
            Settled::Fatal(_) => Lane::Fatal,
        }
    }

    pub fn rejected(self) -> Result<D, SettledFault<L>> {
        match self {
            Settled::Rejected(d) => Ok(d),
            Settled::Denied(d) => Err(SettledFault::Denied(d)),
            Settled::Exhausted(e) => Err(SettledFault::Exhausted(e)),
            Settled::Fatal(f) => Err(SettledFault::Fatal(f)),
        }
    }

    pub fn into_fail<M: LaneProfile>(self) -> Fail<D, M>
    where
        L::Denied: Into<M::Denied>,
        L::Fatal: Into<M::Fatal>,
        <L::Transient as TransientSlot>::Exhausted: ExhaustionInto<M::Fatal>,
    {
        match self {
            Self::Rejected(d) => Fail::Rejected(d),
            Self::Denied(d) => Fail::Denied(d.into()),
            Self::Exhausted(e) => Fail::Fatal(e.exhaustion_into()),
            Self::Fatal(f) => Fail::Fatal(f.into()),
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

impl<D: fmt::Display, L: LaneProfile> fmt::Display for Settled<D, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Settled::Rejected(d) => write!(f, "rejected: {d}"),
            Settled::Denied(d) => write!(f, "{d}"),
            Settled::Exhausted(e) => write!(f, "{e}"),
            Settled::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<D: Error + 'static, L: LaneProfile> Error for Settled<D, L> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Settled::Rejected(d) => Some(d),
            Settled::Denied(d) => Some(d),
            Settled::Exhausted(e) => Some(e),
            Settled::Fatal(x) => Some(x),
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
/// its lane, and how to settle it. Implemented by every [`Failure`] (which
/// covers `Fail<D, L>` itself and every carrier) and by [`Fault<L>`] — the two
/// shapes a generated repo op can return.
pub trait Laned: sealed::Sealed + Error + Send + Sync + 'static + Sized {
    type Settled: Error + Send + Sync + 'static;

    fn lane(&self) -> Lane;

    fn is_transient(&self) -> bool {
        self.lane() == Lane::Transient
    }

    fn settle(self, attempts: u32) -> Self::Settled;
}

impl<F: Failure> Laned for F {
    type Settled = Settled<F::Rejection, F::Lanes>;

    fn lane(&self) -> Lane {
        Failure::lane(self)
    }

    fn settle(self, attempts: u32) -> Self::Settled {
        self.into_fail().settle(attempts)
    }
}

impl<L: LaneProfile> Laned for Fault<L> {
    type Settled = SettledFault<L>;

    fn lane(&self) -> Lane {
        Fault::lane(self)
    }

    fn settle(self, attempts: u32) -> Self::Settled {
        Fault::settle(self, attempts)
    }
}

/// Target-inferred conversions for canonical failure results.
pub trait ResultExt<T, R, L: LaneProfile>: Sized {
    fn widen<P: From<R>, M: LaneProfile>(self) -> Result<T, Fail<P, M>>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>;
    fn lift<P: Lift<R>, M: LaneProfile>(self) -> Result<T, Fail<P, M>>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
        P::Unmapped: UnmappedInto<M::Fatal>;
}
impl<T, R, L: LaneProfile> ResultExt<T, R, L> for Result<T, Fail<R, L>> {
    fn widen<P: From<R>, M: LaneProfile>(self) -> Result<T, Fail<P, M>>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
    {
        self.map_err(Fail::widen)
    }
    fn lift<P: Lift<R>, M: LaneProfile>(self) -> Result<T, Fail<P, M>>
    where
        L::Denied: Into<M::Denied>,
        L::Transient: Into<M::Transient>,
        L::Fatal: Into<M::Fatal>,
        P::Unmapped: UnmappedInto<M::Fatal>,
    {
        self.map_err(Fail::lift)
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
    fn settle_turns_transient_into_exhausted_and_leaves_everything_else_alone() {
        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        match t.settle(3) {
            Settled::Exhausted(e) => assert_eq!(e.attempts, 3),
            other => panic!("expected Exhausted, got {other:?}"),
        }

        let r: Fail<Small> = Fail::Rejected(Small);
        assert!(matches!(r.settle(1), Settled::Rejected(Small)));

        let fatal: Fail<Small> = Fatal::new(FatalKind::Config).into();
        assert!(matches!(fatal.settle(1), Settled::Fatal(_)));
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
    fn laned_settle_agrees_between_fail_and_fault() {
        let t: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
        assert!(matches!(Laned::settle(t, 2), Settled::Exhausted(e) if e.attempts == 2));

        let t: Fault = Transient::new(TransientKind::Deadlock).into();
        assert!(matches!(Laned::settle(t, 2), SettledFault::Exhausted(e) if e.attempts == 2));
    }
}
