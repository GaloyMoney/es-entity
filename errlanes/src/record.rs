use crate::carrier::{LaneRef, LaneRejected};
use crate::dynamic::message_chain;
use crate::fail::Level;

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
    span.record("error.code", lane_code(&lane));
    match lane {
        LaneRef::Rejected(_) | LaneRef::Denied(_) => {}
        LaneRef::Transient(t) => {
            span.record("exception.message", message_chain(t));
        }
        LaneRef::Fatal(x) => {
            span.record("exception.message", message_chain(x));
            span.record("exception.type", x.kind.as_str());
        }
    }
}

/// `Rejected`: `Rejection::code`; `Denied`: `"FORBIDDEN"`; `Transient` /
/// `Fatal`: the kind. Never derived from any `Display`.
fn lane_code<R: LaneRejected>(lane: &LaneRef<'_, R>) -> &'static str {
    match lane {
        LaneRef::Rejected(d) => d.code_str(),
        LaneRef::Denied(_) => "FORBIDDEN",
        LaneRef::Transient(t) => t.kind.as_str(),
        LaneRef::Fatal(x) => x.kind.as_str(),
    }
}

/// Emits one event for `lane`, at the lane's own level ([`LaneRef::level`]) —
/// not the level the enclosing function was instrumented at. Carries the
/// same `error`, `error.lane` and `error.code` as [`record_lanes`], plus
/// `exception.message` as [`LaneRef::message`] builds it: the code alone for
/// `Rejected`, the `Denied` display (no caller text, no retained source) for
/// `Denied`, the operator-safe source chain for `Transient` and `Fatal`.
pub(crate) fn emit_lanes<R: LaneRejected>(lane: LaneRef<'_, R>) {
    let lane_name = lane.lane().as_str();
    let code = lane_code(&lane);
    let message = lane.message();
    match lane.level() {
        Level::Error => tracing::error!(
            error = true,
            error.lane = lane_name,
            error.code = code,
            exception.message = %message
        ),
        Level::Warn => tracing::warn!(
            error = true,
            error.lane = lane_name,
            error.code = code,
            exception.message = %message
        ),
        Level::Info => tracing::info!(
            error = true,
            error.lane = lane_name,
            error.code = code,
            exception.message = %message
        ),
        Level::Debug => tracing::debug!(
            error = true,
            error.lane = lane_name,
            error.code = code,
            exception.message = %message
        ),
        Level::Trace => tracing::trace!(
            error = true,
            error.lane = lane_name,
            error.code = code,
            exception.message = %message
        ),
    }
}

/// Emits the event described on [`emit_lanes`] for `err`. What
/// `#[errlanes::instrument]` calls after [`crate::Laned::record`]; call it
/// directly at a boundary that records by hand.
pub fn emit<E: crate::Laned>(err: &E) {
    err.emit_event();
}
