use std::{borrow::Cow, error::Error, fmt, sync::Arc};

use crate::fail::Level;

/// Which of the four lanes an outcome travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    Rejected,
    Denied,
    Transient,
    Fatal,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Rejected => "rejected",
            Lane::Denied => "denied",
            Lane::Transient => "transient",
            Lane::Fatal => "fatal",
        }
    }
}

impl Lane {
    /// The lane's operator level. `Rejected` is the lane default; a concrete
    /// rejection overrides it through [`crate::Rejection::level`].
    pub fn level(self) -> Level {
        match self {
            Lane::Rejected => Level::Warn,
            Lane::Denied => Level::Warn,
            Lane::Transient => Level::Info,
            Lane::Fatal => Level::Error,
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a [`Transient`] outcome may succeed on retry.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransientKind {
    OptimisticConflict,
    SerializationFailure,
    Deadlock,
    PoolTimeout,
    ConnectionLost,
    UpstreamUnavailable,
    Congestion,
    Other,
}

impl TransientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransientKind::OptimisticConflict => "optimistic_conflict",
            TransientKind::SerializationFailure => "serialization_failure",
            TransientKind::Deadlock => "deadlock",
            TransientKind::PoolTimeout => "pool_timeout",
            TransientKind::ConnectionLost => "connection_lost",
            TransientKind::UpstreamUnavailable => "upstream_unavailable",
            TransientKind::Congestion => "congestion",
            TransientKind::Other => "other",
        }
    }

    /// A transient that carries no evidence about the operation itself, only
    /// about shared capacity — a retry loop backs off without spending its
    /// budget, where a conflict kind (`Deadlock`, `SerializationFailure`,
    /// `OptimisticConflict`) is evidence the operation can be re-run at
    /// once.
    pub fn is_congestion(self) -> bool {
        matches!(self, TransientKind::PoolTimeout | TransientKind::Congestion)
    }

    /// Postgres aborted the attempt because it lost a race with a concurrent
    /// transaction: a deadlock victim (`40P01`) or a serialization failure
    /// (`40001`). Two things follow that no other transient guarantees. The
    /// server confirmed the rollback, so the attempt is safe to re-run even
    /// when the failure surfaced at `COMMIT`, where any other error is
    /// ambiguous. And the failure says nothing about the data involved, only
    /// about the interleaving, so there is nothing in it to attribute to one
    /// row or to bisect for.
    ///
    /// `OptimisticConflict` is deliberately not contention: it is raised
    /// above Postgres by a version check and names one stale row, so a batch
    /// search *can* isolate it, and a plain re-run with the same stale state
    /// would only conflict again.
    pub fn is_contention(self) -> bool {
        matches!(
            self,
            TransientKind::Deadlock | TransientKind::SerializationFailure
        )
    }

    /// The one row of the sqlx lane table (`sqlx.rs`, private) a consumer
    /// with a non-sqlx Postgres driver could still want: which
    /// [`TransientKind`] a Postgres SQLSTATE code maps to, independent of
    /// `sqlx::Error`. Lives here, not behind the `sqlx` feature, so it is
    /// reachable without that dependency.
    pub fn from_sqlstate(code: &str) -> Option<Self> {
        match code {
            "40001" => Some(TransientKind::SerializationFailure),
            "40P01" => Some(TransientKind::Deadlock),
            "57P01" | "57P02" | "57P03" | "08000" | "08003" | "08006" | "08001" | "08004" => {
                Some(TransientKind::ConnectionLost)
            }
            _ => None,
        }
    }
}

impl fmt::Display for TransientKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure that may succeed if the same operation is retried.
#[derive(Debug, Clone)]
pub struct Transient {
    pub kind: TransientKind,
    /// A non-PII breadcrumb, e.g. `"customers/<id> seq 42"`.
    pub context: Option<Cow<'static, str>>,
    source: Option<Arc<dyn Error + Send + Sync + 'static>>,
}

impl Transient {
    pub fn new(kind: TransientKind) -> Self {
        Self {
            kind,
            context: None,
            source: None,
        }
    }

    /// [`Transient::new`] plus [`Transient::with_source`], as one call — the
    /// common shape at a classification site that has the error in hand.
    pub fn from_error(kind: TransientKind, e: impl Error + Send + Sync + 'static) -> Self {
        Self::new(kind).with_source(e)
    }

    pub fn with_source(mut self, e: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(e));
        self
    }

    pub fn with_context(mut self, c: impl Into<Cow<'static, str>>) -> Self {
        self.context = Some(c.into());
        self
    }

    pub fn source_arc(&self) -> Option<&Arc<dyn Error + Send + Sync>> {
        self.source.as_ref()
    }

    pub fn is_congestion(&self) -> bool {
        self.kind.is_congestion()
    }

    pub fn is_contention(&self) -> bool {
        self.kind.is_contention()
    }
}

impl fmt::Display for Transient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transient({})", self.kind)?;
        if let Some(ctx) = &self.context {
            write!(f, ": {ctx}")?;
        }
        Ok(())
    }
}

impl Error for Transient {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|s| s.as_ref() as &(dyn Error + 'static))
    }
}

/// Why a [`Fatal`] outcome is a bug, a misconfiguration, or corrupt state.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FatalKind {
    Invariant,
    Config,
    CorruptState,
    Dependency,
    Panic,
    Exhausted,
    /// A `Denied` narrowed at a boundary with no subject to deny — code
    /// running as the system. The `Denied` is the source. Mirrors
    /// `Exhausted`: just as the struct `Exhausted` is the source of a
    /// `Fatal(Exhausted)`, the struct `Denied` is the source of a
    /// `Fatal(Denied)`.
    Denied,
}

impl FatalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FatalKind::Invariant => "invariant",
            FatalKind::Config => "config",
            FatalKind::CorruptState => "corrupt_state",
            FatalKind::Dependency => "dependency",
            FatalKind::Panic => "panic",
            FatalKind::Exhausted => "exhausted",
            FatalKind::Denied => "denied",
        }
    }
}

impl fmt::Display for FatalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failure that will not succeed on retry: a bug, a misconfiguration, or
/// corrupt state.
///
/// `kind`, `context` and `source` are for operators (traces, logs) and for
/// tests (which may `downcast_ref` the source to assert what happened).
/// Production code never inspects a `Fatal`'s payload: the response to a
/// `Fatal` is the same regardless of its source — stop, surface, page. If you
/// find yourself needing the payload, the outcome was a value or a
/// `Rejection` and the API should be changed, not the call site.
#[derive(Debug, Clone)]
pub struct Fatal {
    pub kind: FatalKind,
    pub context: Option<Cow<'static, str>>,
    source: Option<Arc<dyn Error + Send + Sync + 'static>>,
    /// Set only by a narrowing that stores a domain value (a `Rejection`)
    /// as `source` purely for programmatic access (`downcast_ref` in a
    /// handler or a test) — never for display. `Rejection::Display` is
    /// documented as allowed to embed caller-supplied input, so
    /// [`crate::dynamic::message_chain`] must not walk past this `Fatal`
    /// into it; `message()`/`record` would otherwise leak that text into an
    /// operator-facing field. Not part of equality or ordering; `source()`
    /// and `downcast_ref` are unaffected either way.
    opaque_source: bool,
}

impl Fatal {
    pub fn new(kind: FatalKind) -> Self {
        Self {
            kind,
            context: None,
            source: None,
            opaque_source: false,
        }
    }

    /// [`Fatal::new`] plus [`Fatal::with_source`], as one call — the common
    /// shape at a classification site that has the error in hand.
    pub fn from_error(kind: FatalKind, e: impl Error + Send + Sync + 'static) -> Self {
        Self::new(kind).with_source(e)
    }

    /// As [`Transient::with_source`]. Takes `self` so a `Fatal` built from a
    /// lane table can be given its source afterwards.
    pub fn with_source(mut self, e: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(e));
        self
    }

    /// Explicit escape hatch for a source that only arrives boxed. There is
    /// no `From<Box<dyn Error>>` — a caller must say out loud that it is
    /// discarding whatever lane the boxed error might have carried.
    pub fn from_boxed(kind: FatalKind, e: Box<dyn Error + Send + Sync>) -> Self {
        Self {
            kind,
            context: None,
            source: Some(Arc::from(e)),
            opaque_source: false,
        }
    }

    /// `Fatal::new(kind)` with the error's whole `Display` chain as context —
    /// the by-reference form for a boundary holding a `&dyn Error` it cannot
    /// keep as `source`. The same fold [`Fault::classify`](crate::Fault::classify)
    /// makes in its rule 3, with the kind chosen by the caller.
    pub fn from_dyn(kind: FatalKind, e: &(dyn Error + 'static)) -> Self {
        Self::new(kind).with_context(crate::dynamic::message_chain(e))
    }

    pub fn invariant(msg: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind: FatalKind::Invariant,
            context: Some(msg.into()),
            source: None,
            opaque_source: false,
        }
    }

    pub fn with_context(mut self, c: impl Into<Cow<'static, str>>) -> Self {
        self.context = Some(c.into());
        self
    }

    /// Marks this `Fatal`'s `source` opaque to display: `source()` still
    /// returns it (so a handler or test can still `downcast_ref` it), but
    /// [`crate::dynamic::message_chain`] stops at this `Fatal`'s own
    /// `Display` rather than walking into it. For a narrowing that stores a
    /// `Rejection` as the source — see the field doc on `opaque_source`.
    pub(crate) fn with_opaque_source(mut self) -> Self {
        self.opaque_source = true;
        self
    }

    pub(crate) fn has_opaque_source(&self) -> bool {
        self.opaque_source
    }
}

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fatal({})", self.kind)?;
        if let Some(ctx) = &self.context {
            write!(f, ": {ctx}")?;
        }
        Ok(())
    }
}

impl Error for Fatal {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|s| s.as_ref() as &(dyn Error + 'static))
    }
}

/// Authorization failure. Never retried, always audited.
#[derive(Debug, Clone, Default)]
pub struct Denied {
    pub object: Option<Cow<'static, str>>,
    pub action: Option<Cow<'static, str>>,
    context: Option<Cow<'static, str>>,
    source: Option<Arc<dyn Error + Send + Sync + 'static>>,
}

impl Denied {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_error(e: impl Error + Send + Sync + 'static) -> Self {
        Self::new().with_source(e)
    }

    pub fn with_object(mut self, object: impl Into<Cow<'static, str>>) -> Self {
        self.object = Some(object.into());
        self
    }

    pub fn with_action(mut self, action: impl Into<Cow<'static, str>>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn with_source(mut self, e: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(e));
        self
    }

    pub fn with_context(mut self, c: impl Into<Cow<'static, str>>) -> Self {
        self.context = Some(c.into());
        self
    }

    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    pub fn source_arc(&self) -> Option<&Arc<dyn Error + Send + Sync>> {
        self.source.as_ref()
    }

    pub(crate) fn into_fatal(mut self) -> Fatal {
        let context = self.context.take();
        let fatal = Fatal::from_error(FatalKind::Denied, self);
        match context {
            Some(c) => fatal.with_context(c),
            None => fatal,
        }
    }
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("denied")?;
        if self.action.is_some() || self.object.is_some() {
            f.write_str(": ")?;
            if let Some(action) = &self.action {
                write!(f, "{action} ")?;
            }
            if let Some(object) = &self.object {
                write!(f, "on {object}")?;
            }
        }
        Ok(())
    }
}

impl Error for Denied {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|s| s.as_ref() as &(dyn Error + 'static))
    }
}

/// A [`Transient`] lane that never succeeded within the retry budget.
#[derive(Debug, Clone)]
pub struct Exhausted {
    pub attempts: u32,
    pub last: Transient,
}

impl fmt::Display for Exhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "exhausted after {} attempts: {}",
            self.attempts, self.last
        )
    }
}

impl Error for Exhausted {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `from_sqlstate` must be reachable with no `sqlx` feature
    /// at all -- it lives in this unconditionally-compiled module precisely
    /// so a non-sqlx Postgres driver can classify a SQLSTATE without taking
    /// on the `sqlx` dependency. Compiling this module (this test included)
    /// under the default feature set, which does not enable `sqlx`, is the
    /// proof.
    #[test]
    fn from_sqlstate_is_reachable_without_the_sqlx_feature() {
        assert_eq!(
            TransientKind::from_sqlstate("40P01"),
            Some(TransientKind::Deadlock)
        );
        assert_eq!(TransientKind::from_sqlstate("not-a-code"), None);
    }

    #[test]
    fn contention_is_the_server_confirmed_abort_subset_of_transient() {
        assert!(TransientKind::Deadlock.is_contention());
        assert!(TransientKind::SerializationFailure.is_contention());
        assert!(!TransientKind::OptimisticConflict.is_contention());
        assert!(!TransientKind::ConnectionLost.is_contention());
        assert!(!TransientKind::PoolTimeout.is_contention());
        assert!(!TransientKind::Congestion.is_contention());
        assert!(!TransientKind::Other.is_contention());
        // Exactly the two SQLSTATEs a commit-time retry may trust.
        assert!(
            TransientKind::from_sqlstate("40P01")
                .unwrap()
                .is_contention()
        );
        assert!(
            TransientKind::from_sqlstate("40001")
                .unwrap()
                .is_contention()
        );
        assert!(
            !TransientKind::from_sqlstate("08006")
                .unwrap()
                .is_contention()
        );
        assert!(
            !TransientKind::from_sqlstate("57P01")
                .unwrap()
                .is_contention()
        );
    }
}
