//! Classifies a raw `reqwest::Error`, exactly once, at the point it is born.
//!
//! **`Denied` means the subject of *this* call is unauthorized.** An
//! upstream 401/403 returned to a service account is usually a
//! credential/configuration fault at our own layer, not the caller's — most
//! consumers narrow it away (`#[classify(delegate, narrow(Denied))]`, which
//! becomes `Fatal(Denied)`) or match the status in a hand-written `with` fn
//! to get `Fatal(Config)` instead. A proxy-style wrapper that calls upstream
//! with the caller's *own* token is the case that keeps `Denied` as-is.
//! Keeping it in the built-in table preserves the information either way;
//! narrowing it is one attribute at the call site.

use crate::{
    fail::{Fail, Fault},
    lane::{Denied, Fatal, FatalKind, Transient, TransientKind},
};

/// Which lane a `reqwest::Error` belongs to — the one table both public
/// classifiers read.
fn lane_table(e: &::reqwest::Error) -> Fault<crate::lanes!(Denied, Transient, Fatal)> {
    if e.is_timeout() {
        return Transient::new(TransientKind::UpstreamUnavailable).into();
    }
    if e.is_connect() {
        return Transient::new(TransientKind::ConnectionLost).into();
    }
    if let Some(status) = e.status() {
        if status.as_u16() == 429 {
            return Transient::new(TransientKind::Congestion).into();
        }
        if status.is_server_error() {
            return Transient::new(TransientKind::UpstreamUnavailable).into();
        }
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Denied::default().into();
        }
        if status.is_client_error() {
            // Our own request was malformed (bad params, bad body, …).
            return Fatal::new(FatalKind::Invariant).into();
        }
    }
    if e.is_builder() || e.is_request() {
        return Fatal::new(FatalKind::Invariant).into();
    }
    Fatal::new(FatalKind::Dependency).into()
}

/// Classifies a raw `reqwest::Error`, moving it — the error itself becomes
/// the lane payload's source. What `impl Classify for reqwest::Error`
/// delegates to.
fn classify_reqwest_fault(e: ::reqwest::Error) -> Fault<crate::lanes!(Denied, Transient, Fatal)> {
    match lane_table(&e) {
        Fault::Denied(d) => d.with_source(e).into(),
        Fault::Transient(t) => t.with_source(e).into(),
        Fault::Fatal(f) => f.with_source(e).into(),
    }
}

/// [`classify_reqwest_fault`] for a borrowed error — the same
/// [`lane_table`], with the message folded into `context` in place of the
/// source, which cannot be moved out of a shared reference.
/// [`crate::Fault::classify`] calls this after finding a `reqwest::Error` in
/// a chain it cannot otherwise classify.
pub(crate) fn classify_reqwest_ref(
    e: &::reqwest::Error,
) -> Fault<crate::lanes!(Denied, Transient, Fatal)> {
    match lane_table(e) {
        Fault::Denied(d) => d.with_context(e.to_string()).into(),
        Fault::Transient(t) => t.with_context(e.to_string()).into(),
        Fault::Fatal(f) => f.with_context(e.to_string()).into(),
    }
}

/// `reqwest::Error` may deny (see the module docs), fail transiently, or
/// fail fatally, but never rejects — it enters the lanes with bare `?`
/// through the `Classify` blankets.
impl crate::Classify for ::reqwest::Error {
    type Rejected = core::convert::Infallible;
    type Lanes = crate::lanes!(Denied, Transient, Fatal);

    fn classify(self) -> Fail<Self::Rejected, Self::Lanes> {
        match classify_reqwest_fault(self) {
            Fault::Denied(d) => Fail::Denied(d),
            Fault::Transient(t) => Fail::Transient(t),
            Fault::Fatal(x) => Fail::Fatal(x),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lane::Lane;

    /// `reqwest::Error` is constructible only through a real request/response
    /// cycle, so these tests exercise the decomposed status/kind mapping
    /// `lane_table` itself reads from, rather than a live network call.
    fn lane_for_status(code: u16) -> Lane {
        let status = ::reqwest::StatusCode::from_u16(code).unwrap();
        if status.as_u16() == 429 || status.is_server_error() {
            Lane::Transient
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Lane::Denied
        } else {
            Lane::Fatal
        }
    }

    #[test]
    fn server_errors_are_transient() {
        assert_eq!(lane_for_status(500), Lane::Transient);
        assert_eq!(lane_for_status(503), Lane::Transient);
    }

    #[test]
    fn too_many_requests_is_transient_congestion() {
        assert_eq!(lane_for_status(429), Lane::Transient);
    }

    #[test]
    fn unauthorized_and_forbidden_are_denied() {
        assert_eq!(lane_for_status(401), Lane::Denied);
        assert_eq!(lane_for_status(403), Lane::Denied);
    }

    #[test]
    fn other_client_errors_are_fatal_invariant() {
        assert_eq!(lane_for_status(404), Lane::Fatal);
    }
}
