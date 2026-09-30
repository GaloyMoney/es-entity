use crate::fail::{Fail, Fault, Level, Rejection, Settled, SettledFault};
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
/// `exception.message` is written only for `Fatal`/`Exhausted`, where the
/// message is operator-facing by construction.
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
        }
        Fault::Fatal(x) => {
            span.record("error.code", x.marker().kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.marker().kind.as_str());
        }
    }
}

/// [`record`] for a [`SettledFault`].
pub fn record_settled_fault<L: LaneProfile>(span: &tracing::Span, f: &SettledFault<L>) {
    span.record("error", true);
    span.record("error.lane", f.lane().as_str());
    match f {
        SettledFault::Denied(_) => {
            span.record("error.code", "FORBIDDEN");
            span.record("error.level", "WARN");
        }
        SettledFault::Exhausted(_) => {
            span.record("error.code", "EXHAUSTED");
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", "EXHAUSTED");
        }
        SettledFault::Fatal(x) => {
            span.record("error.code", x.marker().kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.marker().kind.as_str());
        }
    }
}

pub fn record<D: Rejection, L: LaneProfile>(span: &tracing::Span, f: &Settled<D, L>) {
    span.record("error", true);
    span.record("error.lane", f.lane().as_str());
    match f {
        Settled::Rejected(d) => {
            span.record("error.code", Into::<&'static str>::into(d.code()));
            span.record("error.level", level_str(d.level()));
        }
        Settled::Denied(_) => {
            span.record("error.code", "FORBIDDEN");
            span.record("error.level", "WARN");
        }
        Settled::Exhausted(_) => {
            span.record("error.code", "EXHAUSTED");
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", "EXHAUSTED");
        }
        Settled::Fatal(x) => {
            span.record("error.code", x.marker().kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.marker().kind.as_str());
        }
    }
}
