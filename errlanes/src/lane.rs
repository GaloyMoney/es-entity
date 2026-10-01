use std::{borrow::Cow, error::Error, fmt, sync::Arc};

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
}

impl Fatal {
    pub fn new(kind: FatalKind) -> Self {
        Self {
            kind,
            context: None,
            source: None,
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
        }
    }

    pub fn with_context(mut self, c: impl Into<Cow<'static, str>>) -> Self {
        self.context = Some(c.into());
        self
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

impl Error for Denied {}

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
