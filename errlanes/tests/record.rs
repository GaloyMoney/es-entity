#![cfg(feature = "tracing")]
//! `Laned::record` is the single write path every boundary recorder (a
//! `#[errlanes::instrument]`'d fn, a batch dispatcher via
//! `ResultExt::record`) goes through. This exercises it directly with a
//! capturing subscriber, asserting the exact fields `FIELDS` promises for
//! each lane.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use errlanes::{Denied, Fail, Fatal, FatalKind, Laned, ResultExt, Transient, TransientKind};
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::SubscriberExt};

#[derive(Debug, Clone, errlanes::Rejection)]
enum Small {
    #[rejection(code = "SMALL")]
    Unit,
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<HashMap<String, String>>>);

impl Captured {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
}

struct CaptureVisitor(Arc<Mutex<HashMap<String, String>>>);

impl Visit for CaptureVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .lock()
            .unwrap()
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

struct CaptureLayer(Arc<Mutex<HashMap<String, String>>>);

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_record(
        &self,
        _id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = CaptureVisitor(self.0.clone());
        values.record(&mut visitor);
    }
}

/// Declares exactly the six `FIELDS` as `Empty`, then records `failure` onto
/// that span through `Laned::record` under a subscriber that captures every
/// value written.
fn record<E: Laned>(failure: E) -> Captured {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.0.clone()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "boundary",
            error = tracing::field::Empty,
            error.lane = tracing::field::Empty,
            error.code = tracing::field::Empty,
            error.level = tracing::field::Empty,
            exception.message = tracing::field::Empty,
            exception.type = tracing::field::Empty,
        );
        failure.record(&span);
    });
    captured
}

#[test]
fn record_declares_exactly_the_fields_constant() {
    assert_eq!(errlanes::FIELDS.len(), 6);
}

#[test]
fn record_fills_every_field_per_lane() {
    let rejected: Fail<Small> = Fail::Rejected(Small::Unit);
    let captured = record(rejected);
    assert_eq!(captured.get("error").as_deref(), Some("true"));
    assert_eq!(captured.get("error.lane").as_deref(), Some("rejected"));
    assert_eq!(captured.get("error.code").as_deref(), Some("SMALL"));
    assert_eq!(captured.get("error.level").as_deref(), Some("INFO"));
    assert!(
        captured.get("exception.message").is_none(),
        "a rejection's message may embed caller-supplied input"
    );

    let denied: Fail<Small> = Denied::default().into();
    let captured = record(denied);
    assert_eq!(captured.get("error.lane").as_deref(), Some("denied"));
    assert_eq!(captured.get("error.code").as_deref(), Some("FORBIDDEN"));
    assert_eq!(captured.get("error.level").as_deref(), Some("WARN"));
    assert!(captured.get("exception.message").is_none());

    let transient: Fail<Small> = Transient::new(TransientKind::Deadlock).into();
    let captured = record(transient);
    assert_eq!(captured.get("error.lane").as_deref(), Some("transient"));
    assert_eq!(captured.get("error.code").as_deref(), Some("deadlock"));
    assert_eq!(captured.get("error.level").as_deref(), Some("INFO"));
    assert!(
        captured.get("exception.message").is_some(),
        "Transient::context is a non-PII breadcrumb by contract, so it is operator-safe"
    );

    let fatal: Fail<Small> = Fatal::new(FatalKind::Invariant).into();
    let captured = record(fatal);
    assert_eq!(captured.get("error.lane").as_deref(), Some("fatal"));
    assert_eq!(captured.get("error.code").as_deref(), Some("invariant"));
    assert_eq!(captured.get("error.level").as_deref(), Some("ERROR"));
    assert!(captured.get("exception.message").is_some());
    assert_eq!(captured.get("exception.type").as_deref(), Some("invariant"));
}

/// `exception.message` carries the payload's whole `source` chain, not just
/// its own `context` — otherwise a lane built by `classify_sqlx_fault`, which
/// attaches the `sqlx::Error` as a source and usually sets no `context` of
/// its own, records an opaque `fatal(kind)` / `transient(kind)` with no
/// message at all. Both lanes that write the field do this; a `Transient` is
/// the one that gets retried, so an operator reading a retry storm needs its
/// cause just as much.
#[test]
fn exception_message_includes_the_source_chain() {
    let fatal: Fail<Small> =
        Fatal::from_error(FatalKind::Dependency, std::io::Error::other("disk full")).into();
    let message = record(fatal)
        .get("exception.message")
        .expect("Fatal always writes exception.message");
    assert!(
        message.contains("disk full"),
        "expected the source's message in {message:?}"
    );

    let transient: Fail<Small> = Transient::from_error(
        TransientKind::ConnectionLost,
        std::io::Error::other("broken pipe"),
    )
    .into();
    let message = record(transient)
        .get("exception.message")
        .expect("Transient always writes exception.message");
    assert!(
        message.contains("broken pipe"),
        "expected the source's message in {message:?}"
    );
}

#[errlanes::instrument]
fn boundary() -> Result<(), Fail<Small>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

/// `#[errlanes::instrument]` generates its own span (via `#[tracing::instrument]`)
/// and records onto it from inside the fn body, so calling it under a capturing
/// subscriber is enough to see every field `FIELDS` promises, with no span of
/// the caller's own to set up.
#[test]
fn instrumented_fn_records_every_field_on_fatal() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.0.clone()));
    tracing::subscriber::with_default(subscriber, || {
        assert!(boundary().is_err());
    });
    for field in errlanes::FIELDS {
        assert!(captured.get(field).is_some(), "missing {field}");
    }
    assert_eq!(captured.get("error.lane").as_deref(), Some("fatal"));
    assert_eq!(captured.get("error.code").as_deref(), Some("invariant"));
}

#[test]
fn record_result_records_onto_the_current_span_and_returns_self() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(captured.0.clone()));
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "boundary",
            error = tracing::field::Empty,
            error.lane = tracing::field::Empty,
            error.code = tracing::field::Empty,
            error.level = tracing::field::Empty,
            exception.message = tracing::field::Empty,
            exception.type = tracing::field::Empty,
        );
        let _enter = span.enter();
        let result: Result<(), Fail<Small>> = Err(Fail::Rejected(Small::Unit)).record();
        assert!(matches!(result, Err(Fail::Rejected(Small::Unit))));
        let ok: Result<u8, Fail<Small>> = Ok(7).record();
        assert_eq!(ok.unwrap(), 7);
    });
    assert_eq!(captured.get("error.lane").as_deref(), Some("rejected"));
}
