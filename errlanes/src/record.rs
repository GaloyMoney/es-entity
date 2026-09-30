use crate::fail::{Fail, Fault, Level, Rejection};
use crate::profile::{LaneProfile, Slot};

/// Span fields a boundary span must declare as `tracing::field::Empty` for
/// [`record`] / [`record_fail`] to fill.
///
/// ```
/// use errlanes::FIELDS;
///
/// let span = tracing::info_span!(
///     "boundary",
///     error = tracing::field::Empty,
///     error.lane = tracing::field::Empty,
///     error.code = tracing::field::Empty,
///     error.level = tracing::field::Empty,
///     exception.message = tracing::field::Empty,
///     exception.type = tracing::field::Empty,
/// );
/// assert_eq!(FIELDS.len(), 6);
/// ```
pub const FIELDS: &[&str] = &[
    "error",
    "error.lane",
    "error.code",
    "error.level",
    "exception.message",
    "exception.type",
];

fn level_str(level: Level) -> &'static str {
    match level {
        Level::Trace => "TRACE",
        Level::Debug => "DEBUG",
        Level::Info => "INFO",
        Level::Warn => "WARN",
        Level::Error => "ERROR",
    }
}

/// **Display discipline**: `error.code` for a `Rejected` outcome comes from
/// `Rejection::code`, never from `Fail`'s `Display`/`to_string()` — a
/// rejection's message may embed caller-supplied input, so the *code* is the
/// only thing safe to key metrics, alerts, or a GraphQL error extension on.
/// `exception.message` is written for `Transient` and `Fatal`/`Exhausted`,
/// where the message is operator-safe by construction (`Transient::context`
/// is documented as a non-PII breadcrumb); `Rejected` and `Denied` stay
/// code-only.
pub fn record_fail<D: Rejection, L: LaneProfile>(span: &tracing::Span, f: &Fail<D, L>) {
    span.record("error", true);
    span.record("error.lane", f.lane().as_str());
    match f {
        Fail::Rejected(d) => {
            span.record("error.code", Into::<&'static str>::into(d.code()));
            span.record("error.level", level_str(d.level()));
        }
        Fail::Denied(_) => {
            span.record("error.code", "FORBIDDEN");
            span.record("error.level", "WARN");
        }
        Fail::Transient(t) => {
            span.record("error.code", t.marker().kind.as_str());
            span.record("error.level", "INFO");
            span.record("exception.message", f.to_string());
        }
        Fail::Fatal(x) => {
            span.record("error.code", x.marker().kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.marker().kind.as_str());
        }
    }
}

/// [`record_fail`] for a [`Fault`] — no `Rejected` arm to key a code from.
pub fn record_fault<L: LaneProfile>(span: &tracing::Span, f: &Fault<L>) {
    span.record("error", true);
    span.record("error.lane", f.lane().as_str());
    match f {
        Fault::Denied(_) => {
            span.record("error.code", "FORBIDDEN");
            span.record("error.level", "WARN");
        }
        Fault::Transient(t) => {
            span.record("error.code", t.marker().kind.as_str());
            span.record("error.level", "INFO");
            span.record("exception.message", f.to_string());
        }
        Fault::Fatal(x) => {
            span.record("error.code", x.marker().kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.marker().kind.as_str());
        }
    }
}

/// `record` and `record_settled_fault` are gone: a settled failure *is* a
/// `Fail`/`Fault` whose transient slot is disabled, so [`record_fail`] and
/// [`record_fault`] cover it. An exhausted retry records through the `Fatal`
/// arm as `error.code = "exhausted"` (`FatalKind::Exhausted`), which is also
/// what the separate `"EXHAUSTED"` string used to mean — now spelled
/// consistently with `invariant` / `config` / `corrupt_state`.
pub use record_fail as record;

/// Records onto the current span, then hands the result straight back — for
/// a call site that must record and keep going rather than propagate
/// (`?`), such as a batch dispatcher writing its own `conclusion` after the
/// fact.
pub trait RecordResult {
    fn record(self) -> Self;
}

impl<T, E: crate::fail::Laned> RecordResult for Result<T, E> {
    fn record(self) -> Self {
        if let Err(e) = &self {
            e.record(&tracing::Span::current());
        }
        self
    }
}
