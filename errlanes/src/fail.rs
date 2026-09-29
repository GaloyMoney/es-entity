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

/// The uninhabited code type for `Fail<core::convert::Infallible>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NeverCode {}

impl fmt::Display for NeverCode {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl From<NeverCode> for &'static str {
    fn from(code: NeverCode) -> Self {
        match code {}
    }
}

impl Rejection for core::convert::Infallible {
    type Code = NeverCode;

    fn code(&self) -> NeverCode {
        match *self {}
    }
}

/// Implemented by a repo's generated `{Entity}ConstraintViolation`. Read by
/// [`LiftConstraint`] to translate a constraint the domain names into one of
/// its own rejection variants.
pub trait HasConstraint: Rejection {
    type Constraint: Copy + Eq + fmt::Debug + Into<&'static str>;

    fn constraint(&self) -> Option<Self::Constraint>;
    fn constraint_name(&self) -> Option<&str>;
}

/// Emitted by `#[derive(errlanes::Rejection)]` when `#[rejection(repo = X)]`
/// is given: lifts a repo constraint violation into a domain rejection,
/// demoting anything the domain did not name to [`Fatal`].
pub trait LiftConstraint<X: HasConstraint>: Sized {
    fn lift(cv: X) -> Result<Self, Fatal>;
}

/// The generic view over a domain rejection `D`, before retries have run.
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

    pub fn rejected(self) -> Result<D, Fail<D>> {
        match self {
            Fail::Rejected(d) => Ok(d),
            other => Err(other),
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

impl Fail<core::convert::Infallible> {
    /// A `Fail` that provably carries no rejection converts into any
    /// `Fail<D>`.
    pub fn never<D>(self) -> Fail<D> {
        match self {
            Fail::Rejected(never) => match never {},
            Fail::Denied(d) => Fail::Denied(d),
            Fail::Transient(t) => Fail::Transient(t),
            Fail::Fatal(f) => Fail::Fatal(f),
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

    pub fn rejected(self) -> Result<D, Settled<D>> {
        match self {
            Settled::Rejected(d) => Ok(d),
            other => Err(other),
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
    fn never_converts_into_any_rejection_type() {
        let never: Fail<core::convert::Infallible> = Fatal::new(FatalKind::CorruptState).into();
        let widened: Fail<Small> = never.never();
        assert_eq!(widened.lane(), Lane::Fatal);
    }

    #[test]
    fn rejected_narrows_or_returns_the_original() {
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
}
