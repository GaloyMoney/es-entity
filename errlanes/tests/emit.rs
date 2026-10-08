#![cfg(feature = "tracing")]
//! `#[errlanes::instrument]` emits one event per failure, at the failure's own
//! level — the lane default, or `Rejection::level` for a `Rejected` outcome —
//! not at the level the function was instrumented with.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use errlanes::{Denied, Fail, Fatal, FatalKind, Transient, TransientKind};
use tracing::{
    Level,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::SubscriberExt};

const SENTINEL: &str = "sentinel-caller-input-7c1e";

#[derive(Debug, errlanes::Rejection)]
enum Outcome {
    #[rejection(code = "PLAIN")]
    Plain,
    #[rejection(code = "LOUD", level = "warn")]
    Loud,
    #[rejection(code = "CHATTY", level = "debug")]
    Chatty,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[rejection(code = "ECHOES_INPUT", error = manual)]
#[error("rejected input {0}")]
struct EchoesInput(String);

#[derive(Debug, Clone, PartialEq)]
struct Event {
    level: Level,
    fields: HashMap<String, String>,
}

impl Event {
    fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}

#[derive(Default)]
struct FieldVisitor(HashMap<String, String>);

impl Visit for FieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

struct EventLayer(Arc<Mutex<Vec<Event>>>);

impl<S: tracing::Subscriber> Layer<S> for EventLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.0.lock().unwrap().push(Event {
            level: *event.metadata().level(),
            fields: visitor.0,
        });
    }
}

fn events_of(run: impl FnOnce()) -> Vec<Event> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(EventLayer(events.clone()));
    tracing::subscriber::with_default(subscriber, run);
    events.lock().unwrap().clone()
}

#[errlanes::instrument(skip_all)]
fn plain() -> Result<(), Fail<Outcome>> {
    Err(Fail::Rejected(Outcome::Plain))
}

#[errlanes::instrument(skip_all)]
fn loud() -> Result<(), Fail<Outcome>> {
    Err(Fail::Rejected(Outcome::Loud))
}

#[errlanes::instrument(skip_all)]
fn chatty() -> Result<(), Fail<Outcome>> {
    Err(Fail::Rejected(Outcome::Chatty))
}

#[errlanes::instrument(skip_all)]
fn denied() -> Result<(), Fail<Outcome>> {
    Err(Denied::default().into())
}

#[errlanes::instrument(skip_all)]
fn transient() -> Result<(), Fail<Outcome>> {
    Err(Transient::new(TransientKind::Deadlock).into())
}

#[errlanes::instrument(skip_all)]
fn fatal() -> Result<(), Fail<Outcome>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

#[errlanes::instrument(skip_all)]
async fn fatal_async() -> Result<(), Fail<Outcome>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

#[errlanes::instrument(skip_all)]
fn succeeds() -> Result<(), Fail<Outcome>> {
    Ok(())
}

fn only(events: Vec<Event>) -> Event {
    assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
    events.into_iter().next().unwrap()
}

#[test]
fn each_lane_emits_at_its_own_event_level() {
    let event = only(events_of(|| assert!(plain().is_err())));
    assert_eq!(event.level, Level::WARN);
    assert_eq!(event.get("error"), Some("true"));
    assert_eq!(event.get("error.lane"), Some("rejected"));
    assert_eq!(event.get("error.code"), Some("PLAIN"));

    let event = only(events_of(|| assert!(denied().is_err())));
    assert_eq!(event.level, Level::WARN);
    assert_eq!(event.get("error.lane"), Some("denied"));
    assert_eq!(event.get("error.code"), Some("FORBIDDEN"));

    let event = only(events_of(|| assert!(transient().is_err())));
    assert_eq!(event.level, Level::INFO);
    assert_eq!(event.get("error.lane"), Some("transient"));
    assert_eq!(event.get("error.code"), Some("deadlock"));

    let event = only(events_of(|| assert!(fatal().is_err())));
    assert_eq!(event.level, Level::ERROR);
    assert_eq!(event.get("error.lane"), Some("fatal"));
    assert_eq!(event.get("error.code"), Some("invariant"));
}

#[test]
fn a_rejected_outcome_follows_rejection_level() {
    assert_eq!(only(events_of(|| drop(loud()))).level, Level::WARN);
    assert_eq!(only(events_of(|| drop(chatty()))).level, Level::DEBUG);
}

#[test]
fn an_async_instrumented_fn_emits_too() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let event = only(events_of(|| drop(runtime.block_on(fatal_async()))));
    assert_eq!(event.level, Level::ERROR);
}

#[test]
fn a_success_emits_nothing() {
    assert!(events_of(|| drop(succeeds())).is_empty());
}

#[errlanes::instrument(skip_all)]
fn echoes_input() -> Result<(), Fail<EchoesInput>> {
    Err(Fail::Rejected(EchoesInput(SENTINEL.to_string())))
}

#[errlanes::instrument(skip_all)]
fn denied_with_diagnostics() -> Result<(), Fail<Outcome>> {
    Err(Denied::new()
        .with_action("read")
        .with_object("ledger")
        .with_context(SENTINEL)
        .with_source(std::io::Error::other(SENTINEL))
        .into())
}

#[test]
fn a_rejections_event_message_is_its_code_and_never_its_display() {
    let event = only(events_of(|| drop(echoes_input())));
    assert_eq!(event.get("error.code"), Some("ECHOES_INPUT"));
    assert_eq!(event.get("exception.message"), Some("ECHOES_INPUT"));
    for (name, value) in &event.fields {
        assert!(!value.contains(SENTINEL), "{name} leaked caller input");
    }
}

#[test]
fn a_denied_event_uses_the_denied_display_and_none_of_its_diagnostics() {
    let event = only(events_of(|| drop(denied_with_diagnostics())));
    assert_eq!(event.level, Level::WARN);
    assert_eq!(
        event.get("exception.message"),
        Some("denied: read on ledger")
    );
    for (name, value) in &event.fields {
        assert!(!value.contains(SENTINEL), "{name} leaked a diagnostic");
    }
}

#[errlanes::instrument(skip_all)]
fn inner() -> Result<(), Fail<Outcome>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

#[errlanes::instrument(skip_all)]
fn middle() -> Result<(), Fail<Outcome>> {
    inner()
}

#[errlanes::instrument(skip_all)]
fn outer() -> Result<(), Fail<Outcome>> {
    middle()
}

#[test]
fn nested_instrumented_fns_each_emit() {
    let events = events_of(|| drop(outer()));
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(events.iter().all(|e| e.level == Level::ERROR));
}

#[errlanes::instrument(skip_all, emit = false)]
fn quiet() -> Result<(), Fail<Outcome>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

#[errlanes::instrument(emit = true, skip_all)]
fn explicit_emit() -> Result<(), Fail<Outcome>> {
    Err(Fatal::new(FatalKind::Invariant).into())
}

#[test]
fn emit_false_opts_out_and_emit_true_is_the_default() {
    assert!(events_of(|| drop(quiet())).is_empty());
    assert_eq!(events_of(|| drop(explicit_emit())).len(), 1);
}

#[test]
fn emit_false_still_records_the_span_fields() {
    let recorded = Arc::new(Mutex::new(HashMap::new()));
    struct SpanLayer(Arc<Mutex<HashMap<String, String>>>);
    impl<S: tracing::Subscriber> Layer<S> for SpanLayer {
        fn on_record(
            &self,
            _id: &tracing::span::Id,
            values: &tracing::span::Record<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut visitor = FieldVisitor::default();
            values.record(&mut visitor);
            self.0.lock().unwrap().extend(visitor.0);
        }
    }
    let subscriber = tracing_subscriber::registry().with(SpanLayer(recorded.clone()));
    tracing::subscriber::with_default(subscriber, || drop(quiet()));
    assert_eq!(
        recorded
            .lock()
            .unwrap()
            .get("error.lane")
            .map(String::as_str),
        Some("fatal")
    );
}
