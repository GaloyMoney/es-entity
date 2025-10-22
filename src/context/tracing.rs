use serde::{Deserialize, Serialize};
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingContext {
    pub trace_id: String,
    pub span_id: String,
    pub trace_flags: u8,
    /// W3C traceparent header for easy propagation
    pub traceparent: String,
}

impl TracingContext {
    pub fn current() -> Option<Self> {
        use opentelemetry::trace::TraceContextExt;

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
        let traceparent = format!("00-{}-{}-{:02x}", trace_id, span_id, trace_flags);

        Some(TracingContext {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            trace_flags,
            traceparent,
        })
    }

    pub fn inject_as_parent(&self) {
        use opentelemetry::propagation::TextMapPropagator;
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        let mut carrier = std::collections::HashMap::new();

        carrier.insert("traceparent".to_string(), self.traceparent.clone());

        let propagator = TraceContextPropagator::new();
        let extracted_context = propagator.extract(&carrier);
        let _ = tracing::Span::current().set_parent(extracted_context);
    }
}
