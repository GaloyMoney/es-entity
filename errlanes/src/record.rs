use crate::fail::{Fail, Level, Rejection, Settled};

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

pub fn record_fail<D: Rejection>(span: &tracing::Span, f: &Fail<D>) {
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
            span.record("error.code", t.kind.as_str());
            span.record("error.level", "INFO");
        }
        Fail::Fatal(x) => {
            span.record("error.code", x.kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.kind.as_str());
        }
    }
}

pub fn record<D: Rejection>(span: &tracing::Span, f: &Settled<D>) {
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
            span.record("error.code", x.kind.as_str());
            span.record("error.level", "ERROR");
            span.record("exception.message", f.to_string());
            span.record("exception.type", x.kind.as_str());
        }
    }
}
