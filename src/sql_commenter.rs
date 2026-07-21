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
//! The comment survives Postgres query normalization and appears in
//! `pg_stat_activity` and in Postgres logs (slow query log, `auto_explain`,
//! lock waits), so a statement observed server-side can be matched back to a
//! distributed trace. `pg_stat_statements` retains the comment of the *first*
//! execution seen per query id (comments are excluded from the query id
//! jumble), which yields an exemplar trace per statement shape.
//!
//! # Sampling and the prepared statement cache
//!
//! Annotation only happens when the active span is **sampled**: an un-sampled
//! span is never exported, so its trace id cannot be looked up in the tracing
//! backend and annotating would be pure cost. The cost matters because an
//! annotated statement's text is unique per span — it can never be served
//! from sqlx's per-connection prepared statement cache and is executed
//! non-persistently (a server-side parse + plan per execution). Gating on the
//! sampled flag keeps full prepared-statement reuse for all un-sampled
//! traffic.
//!
//! Annotation requires the `tracing-context` feature and an initialized
//! OpenTelemetry tracing layer. Otherwise [`annotate_sql`] is a zero-cost
//! pass-through.

use std::borrow::Cow;

/// The W3C `trace-flags` sampled bit.
#[cfg(feature = "tracing-context")]
const SAMPLED: u8 = 0x01;

/// W3C `traceparent` of the currently active span, e.g.
/// `00-<32-hex-trace-id>-<16-hex-span-id>-01`, if that span is sampled.
///
/// Returns `None` when there is no valid span context (outside any span, or
/// tracing/OTEL not initialized), when the span is not sampled (its trace is
/// never exported, so the annotation could not be correlated with anything),
/// or when the `tracing-context` feature is disabled.
#[cfg(feature = "tracing-context")]
pub fn current_traceparent() -> Option<String> {
    crate::context::TracingContext::current()
        .filter(|ctx| ctx.trace_flags & SAMPLED != 0)
        .map(|ctx| ctx.traceparent)
}

/// W3C `traceparent` of the currently active span. Always `None` without the
/// `tracing-context` feature.
#[cfg(not(feature = "tracing-context"))]
pub fn current_traceparent() -> Option<String> {
    None
}

/// Appends the current sampled span's `traceparent` as a SQL comment to `sql`.
///
/// Returns [`Cow::Borrowed`] (no allocation) when there is no sampled span
/// context; callers can use this to skip any annotation-dependent work.
pub fn annotate_sql(sql: &str) -> Cow<'_, str> {
    match current_traceparent() {
        Some(traceparent) => Cow::Owned(format!("{sql} /*traceparent='{traceparent}'*/")),
        None => Cow::Borrowed(sql),
    }
}

#[cfg(all(test, feature = "tracing-context"))]
mod tests {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    use super::*;

    fn subscriber_with_sampler(
        sampler: opentelemetry_sdk::trace::Sampler,
    ) -> tracing::subscriber::DefaultGuard {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_sampler(sampler)
            .build();
        let tracer = provider.tracer("sql-commenter-test");
        tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .set_default()
    }

    #[test]
    fn no_span_context_is_noop() {
        assert!(current_traceparent().is_none());
        assert_eq!(annotate_sql("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn annotates_within_sampled_span() {
        let _subscriber = subscriber_with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn);

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

    #[test]
    fn unsampled_span_is_not_annotated() {
        let _subscriber = subscriber_with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOff);

        let span = tracing::info_span!("test_span");
        let _guard = span.enter();

        assert!(current_traceparent().is_none());
        assert_eq!(annotate_sql("SELECT 1"), "SELECT 1");
    }
}
