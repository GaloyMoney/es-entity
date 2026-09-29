use std::{error::Error, fmt};

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

/// A foreign rejection carrying an opaque discriminator a domain can
/// pattern-match — the general form of "a repo's `{Entity}ConstraintViolation`
/// names a constraint": any Tier-1 boundary (a ledger's own rejection subset,
/// an HTTP client's `{status, code}`, a payment processor's 409 body) can
/// implement this the same way.
///
/// `Key` is a path into the aggregate: a nested aggregate's key names the
/// child constraint through the parent's variant (e.g.
/// `OrderConstraint::OrderItems(OrderItemConstraint::SkuKey)`), so a domain
/// can hoist a specific nested violation without unwrapping it by hand.
pub trait Liftable: Rejection {
    type Key: Copy + Eq + fmt::Debug + Into<&'static str>;

    /// `None` means the discriminator is unknown to the caller — always
    /// demoted to [`Fatal`] by [`Lift::lift`].
    fn key(&self) -> Option<Self::Key>;
}

/// Emitted by `#[derive(errlanes::Rejection)]` when `#[rejection(lift(X))]`
/// is given: lifts a foreign rejection into a domain rejection, demoting
/// anything the domain did not name to [`Fatal`]. Multiple targets belong
/// in one list, `#[rejection(lift(X, Y))]`; each `key` variant then uses
/// `via = X` or `via = Y` to select its target. Repeating `lift(...)` is
/// a duplicate-field error.
pub trait Lift<X: Liftable>: Sized {
    fn lift(x: X) -> Result<Self, Fatal>;
}

/// `Fail` minus the `Rejected` lane: what an operation that cannot reject
/// (nothing about it is the caller's to correct) returns. Reads return
/// `Fault`; writes return `Fail<{Entity}ConstraintViolation>` — the type
/// itself says whether a call can ever hand back a domain outcome.
#[derive(Debug, Clone)]
pub enum Fault {
    Denied(Denied),
    Transient(Transient),
    Fatal(Fatal),
}

impl Fault {
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
    pub fn settle(self, attempts: u32) -> SettledFault {
        match self {
            Fault::Denied(d) => SettledFault::Denied(d),
            Fault::Transient(last) => SettledFault::Exhausted(Exhausted { attempts, last }),
            Fault::Fatal(f) => SettledFault::Fatal(f),
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Denied(d) => write!(f, "{d}"),
            Fault::Transient(t) => write!(f, "{t}"),
            Fault::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl Error for Fault {
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

impl From<Transient> for Fault {
    fn from(t: Transient) -> Self {
        Fault::Transient(t)
    }
}

impl From<Fatal> for Fault {
    fn from(f: Fatal) -> Self {
        Fault::Fatal(f)
    }
}

impl From<Denied> for Fault {
    fn from(d: Denied) -> Self {
        Fault::Denied(d)
    }
}

impl From<Exhausted> for Fault {
    fn from(e: Exhausted) -> Self {
        Fault::Fatal(Fatal::from_error(crate::lane::FatalKind::Exhausted, e))
    }
}

/// The uninhabited "this hook is not configured" default error type
/// (`post_persist_hook`/`post_hydrate_hook`'s bound is `Fault: From<X>`, and
/// the no-hook case sets `X = core::convert::Infallible`) converts trivially.
impl From<core::convert::Infallible> for Fault {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

impl From<Box<dyn Error + Send + Sync>> for Fault {
    fn from(e: Box<dyn Error + Send + Sync>) -> Self {
        Fault::Fatal(Fatal::from_boxed(crate::lane::FatalKind::Dependency, e))
    }
}

/// `Fault` after retries have run: no `Transient` arm.
#[derive(Debug, Clone)]
pub enum SettledFault {
    Denied(Denied),
    Exhausted(Exhausted),
    Fatal(Fatal),
}

impl SettledFault {
    /// `Exhausted` reports as `Lane::Fatal`, as `Settled::lane` does.
    pub fn lane(&self) -> Lane {
        match self {
            SettledFault::Denied(_) => Lane::Denied,
            SettledFault::Exhausted(_) => Lane::Fatal,
            SettledFault::Fatal(_) => Lane::Fatal,
        }
    }
}

impl fmt::Display for SettledFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettledFault::Denied(d) => write!(f, "{d}"),
            SettledFault::Exhausted(e) => write!(f, "{e}"),
            SettledFault::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl Error for SettledFault {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SettledFault::Denied(d) => Some(d),
            SettledFault::Exhausted(e) => Some(e),
            SettledFault::Fatal(x) => Some(x),
        }
    }
}

impl<D> From<SettledFault> for Settled<D> {
    fn from(f: SettledFault) -> Self {
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
pub enum Fail<D> {
    Rejected(D),
    Denied(Denied),
    Transient(Transient),
    Fatal(Fatal),
}

/// The view over a domain rejection `D` after retries have run: no
/// `Transient` arm, so nothing downstream can forget to handle it.
#[derive(Debug, Clone)]
pub enum Settled<D> {
    Rejected(D),
    Denied(Denied),
    Exhausted(Exhausted),
    Fatal(Fatal),
}

impl<D> Fail<D> {
    pub fn lane(&self) -> Lane {
        match self {
            Fail::Rejected(_) => Lane::Rejected,
            Fail::Denied(_) => Lane::Denied,
            Fail::Transient(_) => Lane::Transient,
            Fail::Fatal(_) => Lane::Fatal,
        }
    }

    /// Lane-preserving widening of the rejection type.
    pub fn widen<D2: From<D>>(self) -> Fail<D2> {
        match self {
            Fail::Rejected(d) => Fail::Rejected(d.into()),
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(f) => Fail::Fatal(f),
        }
    }

    /// Widening that may demote a rejection to `Fatal`. The only
    /// lane-changing conversion in the system.
    pub fn widen_with<D2>(self, f: impl FnOnce(D) -> Result<D2, Fatal>) -> Fail<D2> {
        match self {
            Fail::Rejected(d) => match f(d) {
                Ok(d2) => Fail::Rejected(d2),
                Err(fatal) => Fail::Fatal(fatal),
            },
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(fatal) => Fail::Fatal(fatal),
        }
    }

    pub fn map_rejected<D2>(self, f: impl FnOnce(D) -> D2) -> Fail<D2> {
        match self {
            Fail::Rejected(d) => Fail::Rejected(f(d)),
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(fatal) => Fail::Fatal(fatal),
        }
    }

    /// Narrows to the domain outcome, or the non-domain fault. `let d =
    /// e.rejected()?;` propagates the fault into any enclosing `Fail<_>` (or
    /// a `Failure` carrier) via the blanket `From<Fault>`.
    pub fn rejected(self) -> Result<D, Fault> {
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
    pub fn settle(self, attempts: u32) -> Settled<D> {
        match self {
            Fail::Rejected(d) => Settled::Rejected(d),
            Fail::Denied(d) => Settled::Denied(d),
            Fail::Transient(last) => Settled::Exhausted(Exhausted { attempts, last }),
            Fail::Fatal(f) => Settled::Fatal(f),
        }
    }
}

impl<D> Settled<D> {
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

    pub fn rejected(self) -> Result<D, SettledFault> {
        match self {
            Settled::Rejected(d) => Ok(d),
            Settled::Denied(d) => Err(SettledFault::Denied(d)),
            Settled::Exhausted(e) => Err(SettledFault::Exhausted(e)),
            Settled::Fatal(f) => Err(SettledFault::Fatal(f)),
        }
    }

    pub fn into_fail(self) -> Fail<D> {
        match self {
            Settled::Rejected(d) => Fail::Rejected(d),
            Settled::Denied(d) => Fail::Denied(d),
            Settled::Exhausted(e) => {
                Fail::Fatal(Fatal::from_error(crate::lane::FatalKind::Exhausted, e))
            }
            Settled::Fatal(f) => Fail::Fatal(f),
        }
    }
}

/// The real blanket the `Infallible` encoding could not have: `Fault` is not
/// `Fail`, so this does not overlap `From<T> for T`.
impl<D> From<Fault> for Fail<D> {
    fn from(f: Fault) -> Self {
        match f {
            Fault::Denied(d) => Fail::Denied(d),
            Fault::Transient(t) => Fail::Transient(t),
            Fault::Fatal(x) => Fail::Fatal(x),
        }
    }
}

impl<D> From<Transient> for Fail<D> {
    fn from(t: Transient) -> Self {
        Fail::Transient(t)
    }
}

impl<D> From<Fatal> for Fail<D> {
    fn from(f: Fatal) -> Self {
        Fail::Fatal(f)
    }
}

impl<D> From<Denied> for Fail<D> {
    fn from(d: Denied) -> Self {
        Fail::Denied(d)
    }
}

impl<D> From<Exhausted> for Fail<D> {
    fn from(e: Exhausted) -> Self {
        Fail::Fatal(Fatal::from_error(crate::lane::FatalKind::Exhausted, e))
    }
}

impl<D: Rejection> From<D> for Fail<D> {
    fn from(d: D) -> Self {
        Fail::Rejected(d)
    }
}

impl<D: fmt::Display> fmt::Display for Fail<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Rejected(d) => write!(f, "rejected: {d}"),
            Fail::Denied(d) => write!(f, "{d}"),
            Fail::Transient(t) => write!(f, "{t}"),
            Fail::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<D: Error + 'static> Error for Fail<D> {
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

impl<D: fmt::Display> fmt::Display for Settled<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Settled::Rejected(d) => write!(f, "rejected: {d}"),
            Settled::Denied(d) => write!(f, "{d}"),
            Settled::Exhausted(e) => write!(f, "{e}"),
            Settled::Fatal(x) => write!(f, "{x}"),
        }
    }
}

impl<D: Error + 'static> Error for Settled<D> {
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
/// recorders) is written against this, not against `Fail<D>` directly.
pub trait Failure: Error + Send + Sync + Sized + 'static {
    type Rejection: Rejection;

    fn into_fail(self) -> Fail<Self::Rejection>;
    fn from_fail(f: Fail<Self::Rejection>) -> Self;
    fn as_fail(&self) -> &Fail<Self::Rejection>;

    fn lane(&self) -> Lane {
        self.as_fail().lane()
    }

    fn is_transient(&self) -> bool {
        matches!(self.lane(), Lane::Transient)
    }
}

impl<D: Rejection> Failure for Fail<D> {
    type Rejection = D;

    fn into_fail(self) -> Fail<Self::Rejection> {
        self
    }

    fn from_fail(f: Fail<Self::Rejection>) -> Self {
        f
    }

    fn as_fail(&self) -> &Fail<Self::Rejection> {
        self
    }
}

mod sealed {
    pub trait Sealed {}
    impl<F: super::Failure> Sealed for F {}
    impl Sealed for super::Fault {}
}

/// Sealed. The thing `retry`/`record` need from any error they are handed:
/// its lane, and how to settle it. Implemented by every [`Failure`] (which
/// covers `Fail<D>` itself and every carrier) and by [`Fault`] — the two
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
    type Settled = Settled<F::Rejection>;

    fn lane(&self) -> Lane {
        Failure::lane(self)
    }

    fn settle(self, attempts: u32) -> Self::Settled {
        self.into_fail().settle(attempts)
    }
}

impl Laned for Fault {
    type Settled = SettledFault;

    fn lane(&self) -> Lane {
        Fault::lane(self)
    }

    fn settle(self, attempts: u32) -> Self::Settled {
        Fault::settle(self, attempts)
    }
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
