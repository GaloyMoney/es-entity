use crate::carrier::{LaneRef, LaneRejected};
use crate::dynamic::message_chain;

/// Span fields a boundary span must declare as `tracing::field::Empty` for
/// [`crate::Laned::record`] to fill.
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

/// **Display discipline**: `error.code` for a `Rejected` outcome comes from
/// `Rejection::code`, never from `Fail`'s `Display`/`to_string()` — a
/// rejection's message may embed caller-supplied input, so the *code* is the
/// only thing safe to key metrics, alerts, or a GraphQL error extension on.
/// `exception.message` is written for `Transient` and `Fatal`/`Exhausted`,
/// where the message is operator-safe by construction (`Transient::context`
/// is documented as a non-PII breadcrumb); `Rejected` and `Denied` stay
/// code-only. Both carry their whole `source` chain ([`message_chain`]), not
/// just their own `context` — the chain is a sqlx error or another
/// lanes-aware payload, operator-safe by the same contract, and without it a
/// payload that set no `context` of its own records nothing but its kind. A
/// lane built from an arbitrary error whose message embeds caller input
/// should set its own operator-safe `context` instead of relying on the
/// default.
pub(crate) fn record_lanes<R: LaneRejected>(span: &tracing::Span, lane: LaneRef<'_, R>) {
    span.record("error", true);
    span.record("error.lane", lane.lane().as_str());
    span.record("error.level", lane.level().as_str());
    match lane {
        LaneRef::Rejected(d) => {
            span.record("error.code", d.code_str());
        }
        LaneRef::Denied(_) => {
            span.record("error.code", "FORBIDDEN");
        }
        LaneRef::Transient(t) => {
            span.record("error.code", t.kind.as_str());
            span.record("exception.message", message_chain(t));
        }
        LaneRef::Fatal(x) => {
            span.record("error.code", x.kind.as_str());
            span.record("exception.message", message_chain(x));
            span.record("exception.type", x.kind.as_str());
        }
    }
}
