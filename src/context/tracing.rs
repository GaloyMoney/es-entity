use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingContext {
    pub trace_id: String,
    pub span_id: String,
    pub trace_flags: u8,
    /// W3C traceparent header for easy propagation
    pub traceparent: String,
    /// W3C tracestate, including vendor sampling and retention state.
    /// Empty when the producer had no vendor state.
    pub tracestate: String,
}

impl TracingContext {
    #[cfg(feature = "tracing-context")]
    pub fn current() -> Option<Self> {
        use opentelemetry::{propagation::TextMapPropagator, trace::TraceContextExt};
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        let current = tracing::Span::current();
        let context = current.context();

        let span = context.span();
        let span_context = span.span_context();
        if !span_context.is_valid() {
            return None;
        }

        let trace_id = span_context.trace_id();
        let span_id = span_context.span_id();
        let trace_flags =
            (span_context.trace_flags() & opentelemetry::trace::TraceFlags::SAMPLED).to_u8();
        let mut carrier = std::collections::HashMap::new();
        TraceContextPropagator::new().inject_context(&context, &mut carrier);
        let traceparent = carrier.get("traceparent")?.clone();
        let tracestate = carrier.get("tracestate").cloned().unwrap_or_default();

        Some(TracingContext {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            trace_flags,
            traceparent,
            tracestate,
        })
    }

    #[cfg(feature = "tracing-context")]
    fn extract(&self) -> opentelemetry::Context {
        use opentelemetry::propagation::TextMapPropagator;
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        let mut carrier = std::collections::HashMap::new();
        carrier.insert("traceparent".to_string(), self.traceparent.clone());
        if !self.tracestate.is_empty() {
            carrier.insert("tracestate".to_string(), self.tracestate.clone());
        }

        TraceContextPropagator::new().extract_with_context(&opentelemetry::Context::new(), &carrier)
    }

    /// Apply this context to `span` before the span is activated. The
    /// tracing-opentelemetry bridge cannot change a span's parent after its
    /// OTel context has started.
    #[cfg(feature = "tracing-context")]
    pub fn apply_parent(
        &self,
        span: &tracing::Span,
    ) -> Result<(), tracing_opentelemetry::SetParentError> {
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        span.set_parent(self.extract())
    }

    #[cfg(feature = "tracing-context")]
    #[deprecated(
        note = "create the consumer span first and call apply_parent before activating it"
    )]
    pub fn inject_as_parent(&self) {
        let _ = self.apply_parent(&tracing::Span::current());
    }

    /// Add this persisted context as a link on `span`. Unlike a parent, a
    /// link allows one batch span to name every producer whose job
    /// contributed work to that batch. Links may be added before or after
    /// span activation.
    #[cfg(feature = "tracing-context")]
    pub fn apply_link(&self, span: &tracing::Span, attributes: Vec<opentelemetry::KeyValue>) {
        use opentelemetry::trace::TraceContextExt;
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        let context = self.extract();
        let extracted_span = context.span();
        let span_context = extracted_span.span_context();
        if span_context.is_valid() {
            span.add_link_with_attributes(span_context.clone(), attributes);
        }
    }

    #[cfg(feature = "tracing-context")]
    pub fn inject_as_link(&self, attributes: Vec<opentelemetry::KeyValue>) {
        self.apply_link(&tracing::Span::current(), attributes);
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "tracing-context")]
    mod otel {
        use opentelemetry::{
            Context, KeyValue,
            trace::{SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, TracerProvider},
        };
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        use tracing_subscriber::layer::SubscriberExt;

        use crate::TracingContext;

        fn remote_parent() -> Context {
            Context::new().with_remote_span_context(opentelemetry::trace::SpanContext::new(
                TraceId::from_hex("11111111111111111111111111111111").expect("trace id"),
                SpanId::from_hex("2222222222222222").expect("span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::from_key_value([("galoy", "retain"), ("vendor", "state")])
                    .expect("trace state"),
            ))
        }

        fn find_span<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
            spans
                .iter()
                .find(|span| span.name == name)
                .unwrap_or_else(|| panic!("missing {name} span"))
        }

        #[test]
        fn current_parent_and_link_round_trip_tracestate() {
            let exporter = InMemorySpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter.clone())
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("context-test")));
            tracing::subscriber::with_default(subscriber, || {
                let producer = tracing::info_span!("producer");
                producer
                    .set_parent(remote_parent())
                    .expect("parent applies before activation");
                let captured = producer
                    .in_scope(|| TracingContext::current().expect("sampled producer context"));
                assert_eq!(captured.trace_id, "11111111111111111111111111111111");
                assert_eq!(captured.tracestate, "galoy=retain,vendor=state");

                let child = tracing::info_span!(parent: None, "child");
                captured
                    .apply_parent(&child)
                    .expect("persisted parent applies before activation");
                child.in_scope(|| {});
                let linked = tracing::info_span!(parent: None, "linked");
                captured.apply_link(&linked, vec![KeyValue::new("job.id", "job-1")]);
                linked.in_scope(|| {});

                drop(linked);
                drop(child);
                drop(producer);
                provider.force_flush().expect("flush spans");
                let spans = exporter.get_finished_spans().expect("finished spans");

                let child_data = find_span(&spans, "child");
                assert_eq!(child_data.parent_span_id.to_string(), captured.span_id);
                assert_eq!(
                    child_data
                        .span_context
                        .trace_state()
                        .get("galoy")
                        .map(str::to_owned),
                    Some("retain".to_string())
                );

                let linked_data = find_span(&spans, "linked");
                assert_eq!(linked_data.parent_span_id, SpanId::INVALID);
                assert_eq!(linked_data.links.links.len(), 1);
                assert_eq!(linked_data.links.dropped_count, 0);
                let link = &linked_data.links.links[0];
                assert_eq!(link.span_context.trace_id().to_string(), captured.trace_id);
                assert_eq!(link.span_context.span_id().to_string(), captured.span_id);
                assert_eq!(link.span_context.trace_state().get("galoy"), Some("retain"));
                assert_eq!(link.span_context.trace_state().get("vendor"), Some("state"));
                assert!(link.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "job.id"
                        && matches!(&attribute.value, opentelemetry::Value::String(value) if value.as_str() == "job-1")
                }));
            });
        }

        #[test]
        fn invalid_persisted_parent_does_not_create_a_link() {
            let exporter = InMemorySpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(exporter.clone())
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("context-test")));
            tracing::subscriber::with_default(subscriber, || {
                let context = TracingContext {
                    trace_id: "00000000000000000000000000000000".to_string(),
                    span_id: "0000000000000000".to_string(),
                    trace_flags: 1,
                    traceparent: "00-00000000000000000000000000000000-0000000000000000-01"
                        .to_string(),
                    tracestate: "galoy=retain".to_string(),
                };
                let linked = tracing::info_span!("invalid-linked");
                context.apply_link(&linked, Vec::new());
                linked.in_scope(|| {});
                drop(linked);

                provider.force_flush().expect("flush spans");
                let spans = exporter.get_finished_spans().expect("finished spans");
                assert!(find_span(&spans, "invalid-linked").links.links.is_empty());
            });
        }
    }
}
