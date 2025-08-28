use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingContext {
    pub trace_id: String,
    pub span_id: String,
    pub trace_flags: u8,
    /// W3C traceparent header for easy propagation
    pub traceparent: String,
}

pub(super) fn extract_current_tracing_context() -> Option<TracingContext> {
    use opentelemetry::trace::TraceContextExt;
    use tracing::Span;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let current = Span::current();
    let context = current.context();

    let span = context.span();
    let span_context = span.span_context();
    if !span_context.is_valid() {
        return None;
    }

    let trace_id = span_context.trace_id().to_string();
    let span_id = span_context.span_id().to_string();
    let trace_flags = span_context.trace_flags().to_u8();

    let traceparent = format!("00-{}-{}-{:02x}", trace_id, span_id, trace_flags);

    Some(TracingContext {
        trace_id,
        span_id,
        trace_flags,
        traceparent,
    })
}
