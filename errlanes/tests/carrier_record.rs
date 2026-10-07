#![cfg(feature = "tracing")]
//! A carrier records exactly what its built-in records: same fields, same values.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use errlanes::{Denied, Fail, Fatal, FatalKind, Laned, Transient, TransientKind};
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

#[errlanes::fail(Small; Transient, Fatal, Denied)]
struct Carried;

#[errlanes::fault(Transient, Fatal)]
struct CarriedFault;

fn fields(c: &Captured) -> Vec<Option<String>> {
    [
        "error",
        "error.lane",
        "error.code",
        "error.level",
        "exception.message",
        "exception.type",
    ]
    .iter()
    .map(|k| c.get(k))
    .collect()
}

#[test]
fn carrier_records_what_its_builtin_records() {
    let pairs: Vec<(Carried, Fail<Small>)> = vec![
        (Carried::Rejected(Small::Unit), Fail::Rejected(Small::Unit)),
        (Carried::Denied(Denied::default()), Denied::default().into()),
        (
            Carried::Transient(Transient::new(TransientKind::Deadlock)),
            Transient::new(TransientKind::Deadlock).into(),
        ),
        (
            Carried::Fatal(Fatal::new(FatalKind::Config)),
            Fatal::new(FatalKind::Config).into(),
        ),
    ];
    for (carrier, builtin) in pairs {
        assert_eq!(Laned::message(&carrier), Laned::message(&builtin));
        assert_eq!(fields(&record(carrier)), fields(&record(builtin)));
    }

    let fault = CarriedFault::Transient(Transient::new(TransientKind::Deadlock));
    let captured = record(fault);
    assert_eq!(captured.get("error.lane").as_deref(), Some("transient"));
}
