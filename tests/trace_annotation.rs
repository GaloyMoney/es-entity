#![cfg(feature = "tracing-context")]
//! End-to-end proof that statements executed through
//! [`AtomicOperation::as_executor`] carry the active span's `traceparent`
//! comment all the way to Postgres.

mod helpers;

use es_entity::*;
use helpers::init_pool;

#[tokio::test]
async fn trace_context_reaches_postgres() -> anyhow::Result<()> {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
        .build();
    let tracer = provider.tracer("trace-annotation-test");
    let _ = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init();

    // Annotation is disabled by default; this test exercises the opt-in path.
    sql_commenter::set_annotation_enabled(true);

    let pool = init_pool().await?;

    let span = tracing::info_span!("trace_annotation_test");

    // Instrument the future rather than holding a `span.enter()` guard across
    // awaits: `Entered` is thread-local and must not migrate between worker
    // threads on the multi-threaded runtime.
    use tracing::Instrument as _;
    async {
        let traceparent = sql_commenter::current_traceparent().expect("valid span context");
        let trace_id = traceparent.split('-').nth(1).unwrap().to_string();

        // The executor every generated write path uses: the one provided by
        // `AtomicOperation::as_executor()`.
        let mut op = DbOp::init(&pool).await?;

        // Both futures are polled on this task (where the span is the current
        // context), so the slow query is executed with the span active.
        let slow = sqlx::query("SELECT pg_sleep(2)").execute(op.as_executor());
        let poll = async {
            for _ in 0..100 {
                let row: Option<(String,)> = sqlx::query_as(
                    "SELECT query FROM pg_stat_activity \
                     WHERE query LIKE $1 AND pid <> pg_backend_pid()",
                )
                .bind(format!("%traceparent='00-{trace_id}-%"))
                .fetch_optional(&pool)
                .await?;
                if row.is_some() {
                    return Ok::<bool, sqlx::Error>(true);
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Ok(false)
        };

        let (slow_result, found) = tokio::join!(slow, poll);
        slow_result?;
        assert!(
            found?,
            "annotated statement should be visible in pg_stat_activity"
        );
        Ok(())
    }
    .instrument(span)
    .await
}
