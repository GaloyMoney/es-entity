#![cfg(feature = "tracing-context")]
//! Regression tests for annotated statements (`persistent(false)`, i.e. the
//! *unnamed* prepared statement) that reference a custom Postgres type.
//!
//! sqlx-postgres 0.9.0 resolves the OID of a custom type it has not seen on a
//! connection yet with a simple `Query` message, which destroys the unnamed
//! statement it had just parsed; the following `Bind` then fails with
//! SQLSTATE 26000 (`unnamed prepared statement does not exist`). It fails once
//! per connection per type, so each test runs on a brand new single-connection
//! pool: a cold type cache.
//!
//! Annotation needs *both* the global opt-in toggle and a sampled span; with
//! either missing the statement is passed through untouched and runs as a
//! cached named statement, which cannot hit the bug. Each test therefore
//! asserts, from inside Postgres, that the statement it ran really carried the
//! span's `traceparent`.

mod helpers;

use es_entity::*;
use tracing::Instrument as _;

#[derive(Debug, PartialEq, sqlx::Type)]
#[sqlx(type_name = "unnamed_statement_mood", rename_all = "snake_case")]
enum Mood {
    Happy,
    Sad,
}

/// A text parameter sent with an unspecified type (OID 0), so that Postgres
/// infers the parameter's type from the statement. For `$1::<custom type>` the
/// custom type's OID is then only learned from the `Describe` response, which
/// is the path that needs the catalog lookup. A parameter bound with a typed
/// enum (`Mood`) is resolved by name before `Parse` and never reaches it.
struct Unspecified(&'static str);

impl sqlx::Type<sqlx::Postgres> for Unspecified {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Unspecified {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }
}

/// Adds the SQLSTATE to a database error, so a failure names the code.
fn with_sqlstate(e: sqlx::Error) -> anyhow::Error {
    let code = e
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.into_owned()));
    anyhow::anyhow!("{e} (SQLSTATE {})", code.as_deref().unwrap_or("none"))
}

/// Enables annotation and installs a subscriber whose spans are always
/// sampled. Both are process-global; nextest runs each test in its own
/// process.
fn enable_sampled_annotation() {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
        .build();
    let tracer = provider.tracer("unnamed-statement-test");
    let _ = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init();
    sql_commenter::set_annotation_enabled(true);
}

/// A pool holding exactly one, freshly opened connection: its type cache is
/// empty, so the custom type's OID is unknown to it.
async fn cold_pool() -> anyhow::Result<sqlx::PgPool> {
    let pg_con = std::env::var("PG_CON")?;
    Ok(sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&pg_con)
        .await?)
}

/// Asserts that the statement Postgres executed carried the sampled span's
/// `traceparent` annotation. `executed` is `current_query()` as seen by the
/// server.
fn assert_annotated(executed: &str, traceparent: &str) {
    assert!(
        traceparent.ends_with("-01"),
        "the span must be sampled: {traceparent}"
    );
    assert!(
        executed.contains(&format!("/*traceparent='{traceparent}'*/")),
        "statement was not annotated, so it never ran as the unnamed statement: {executed}"
    );
}

#[tokio::test]
async fn annotated_query_with_custom_enum_result_column_on_cold_connection() -> anyhow::Result<()> {
    enable_sampled_annotation();
    let pool = cold_pool().await?;

    async {
        let traceparent = sql_commenter::current_traceparent().expect("sampled span");
        let mut op = DbOp::init(&pool).await?;

        let (mood, executed): (Mood, String) =
            sqlx::query_as("SELECT 'happy'::unnamed_statement_mood, current_query()")
                .fetch_one(op.as_executor())
                .await
                .map_err(with_sqlstate)?;

        assert_eq!(mood, Mood::Happy);
        assert_annotated(&executed, &traceparent);
        Ok(())
    }
    .instrument(tracing::info_span!("unnamed_statement_result_column"))
    .await
}

#[tokio::test]
async fn annotated_query_with_custom_enum_parameter_on_cold_connection() -> anyhow::Result<()> {
    enable_sampled_annotation();
    let pool = cold_pool().await?;

    async {
        let traceparent = sql_commenter::current_traceparent().expect("sampled span");
        let mut op = DbOp::init(&pool).await?;

        // The only custom-typed thing in this statement is the parameter: the
        // result columns are `text`.
        let (mood, executed): (String, String) =
            sqlx::query_as("SELECT $1::unnamed_statement_mood::text, current_query()")
                .bind(Unspecified("sad"))
                .fetch_one(op.as_executor())
                .await
                .map_err(with_sqlstate)?;

        assert_eq!(mood, "sad");
        assert_annotated(&executed, &traceparent);
        Ok(())
    }
    .instrument(tracing::info_span!("unnamed_statement_parameter"))
    .await
}
