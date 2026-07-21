//! Correlate SQL statements with OpenTelemetry traces.
//!
//! [`annotate_sql`] appends a [sqlcommenter](https://google.github.io/sqlcommenter)-style
//! `traceparent` comment carrying the currently active span's trace context to
//! a SQL statement, e.g.
//!
//! ```text
//! SELECT * FROM users WHERE id = $1 /*traceparent='00-<32-hex>-<16-hex>-01'*/
//! ```
//!
//! The comment survives Postgres query normalization and is retained in the
//! representative query text stored by `pg_stat_statements`, and appears in
//! Postgres logs (slow query log, `auto_explain`, lock waits). A statement
//! observed server-side can therefore be matched back to a distributed trace:
//!
//! ```sql
//! SELECT * FROM pg_stat_statements WHERE query LIKE '%<trace_id>%';
//! ```
//!
//! Annotation only happens when there is a valid span context (i.e. tracing
//! with an OpenTelemetry layer is initialized and a span is active) and the
//! `tracing-context` feature is enabled. Otherwise [`annotate_sql`] is a
//! zero-cost pass-through.

use std::borrow::Cow;

/// W3C `traceparent` of the currently active span, e.g.
/// `00-<32-hex-trace-id>-<16-hex-span-id>-01`.
///
/// Returns `None` when there is no valid span context (outside any span, or
/// tracing/OTEL not initialized) or the `tracing-context` feature is disabled.
#[cfg(feature = "tracing-context")]
pub fn current_traceparent() -> Option<String> {
    crate::context::TracingContext::current().map(|ctx| ctx.traceparent)
}

/// W3C `traceparent` of the currently active span. Always `None` without the
/// `tracing-context` feature.
#[cfg(not(feature = "tracing-context"))]
pub fn current_traceparent() -> Option<String> {
    None
}

/// Appends the current span's `traceparent` as a SQL comment to `sql`.
///
/// Returns [`Cow::Borrowed`] (no allocation) when there is no active span
/// context; callers can use this to skip any annotation-dependent work.
pub fn annotate_sql(sql: &str) -> Cow<'_, str> {
    match current_traceparent() {
        Some(traceparent) => Cow::Owned(format!("{sql} /*traceparent='{traceparent}'*/")),
        None => Cow::Borrowed(sql),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tracing-context")]
    #[test]
    fn no_span_context_is_noop() {
        assert!(current_traceparent().is_none());
        assert_eq!(annotate_sql("SELECT 1"), "SELECT 1");
    }

    #[cfg(feature = "tracing-context")]
    #[test]
    fn annotates_within_active_span() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .build();
        let tracer = provider.tracer("sql-commenter-test");
        tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .init();

        let span = tracing::info_span!("test_span");
        let _guard = span.enter();

        let tp = current_traceparent().expect("should have traceparent in span");
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3], "01");

        assert_eq!(
            annotate_sql("SELECT 1"),
            format!("SELECT 1 /*traceparent='{tp}'*/")
        );
    }
}
