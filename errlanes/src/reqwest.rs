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
/// the lane payload's source (or is discarded for `Denied`, which carries no
/// source today). What `impl Classify for reqwest::Error` delegates to.
fn classify_reqwest_fault(e: ::reqwest::Error) -> Fault<crate::lanes!(Denied, Transient, Fatal)> {
    match lane_table(&e) {
        Fault::Denied(d) => Fault::Denied(d),
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
        Fault::Denied(d) => Fault::Denied(d),
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
    use std::error::Error;

    use super::*;

    fn status_error(status: u16) -> ::reqwest::Error {
        ::reqwest::Response::from(http::Response::builder().status(status).body("").unwrap())
            .error_for_status()
            .unwrap_err()
    }

    #[test]
    fn http_statuses_classify_through_the_foreign_error_impl() {
        for status in [500, 503, 429] {
            let fault: Fault<crate::lanes!(Denied, Transient, Fatal)> = status_error(status).into();
            let Fault::Transient(transient) = fault else {
                panic!("expected transient for HTTP {status}: {fault:?}");
            };
            assert_eq!(
                transient.kind,
                if status == 429 {
                    TransientKind::Congestion
                } else {
                    TransientKind::UpstreamUnavailable
                }
            );
            let source = transient
                .source()
                .unwrap()
                .downcast_ref::<::reqwest::Error>()
                .unwrap();
            assert_eq!(source.status().unwrap().as_u16(), status);
        }
        for status in [401, 403] {
            let fault: Fault<crate::lanes!(Denied, Transient, Fatal)> = status_error(status).into();
            assert!(matches!(fault, Fault::Denied(_)));
        }
        for status in [400, 404, 422] {
            let fault: Fault<crate::lanes!(Denied, Transient, Fatal)> = status_error(status).into();
            let Fault::Fatal(fatal) = fault else {
                panic!("expected fatal for HTTP {status}: {fault:?}");
            };
            assert_eq!(fatal.kind, FatalKind::Invariant);
            assert_eq!(
                fatal
                    .source()
                    .unwrap()
                    .downcast_ref::<::reqwest::Error>()
                    .unwrap()
                    .status()
                    .unwrap()
                    .as_u16(),
                status
            );
        }
    }

    #[test]
    fn borrowed_foreign_errors_keep_the_same_lane_at_dynamic_boundaries() {
        let error = status_error(429);
        let fault = Fault::classify(&error);
        assert!(
            matches!(fault, Fault::Transient(transient) if transient.kind == TransientKind::Congestion)
        );
        let error = status_error(404);
        let fault = Fault::classify(&error);
        assert!(matches!(fault, Fault::Fatal(fatal) if fatal.kind == FatalKind::Invariant));
    }

    #[test]
    fn invalid_request_is_an_invariant_with_the_original_source() {
        let error = ::reqwest::Client::new()
            .get("://invalid")
            .build()
            .unwrap_err();
        let fault: Fault<crate::lanes!(Denied, Transient, Fatal)> = error.into();
        let Fault::Fatal(fatal) = fault else {
            panic!("expected fatal: {fault:?}")
        };
        assert_eq!(fatal.kind, FatalKind::Invariant);
        assert!(fatal.source().unwrap().is::<::reqwest::Error>());
    }
}
