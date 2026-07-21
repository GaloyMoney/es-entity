//! Executor wrapper that annotates SQL statements with the current trace
//! context.
//!
//! [`annotate_executor`] wraps any Postgres [`sqlx::Executor`] so that every
//! statement executed through it carries the active span's `traceparent` as a
//! SQL comment (see [`crate::sql_commenter`]). This is what allows statements
//! observed server-side (`pg_stat_statements`, Postgres logs) to be matched to
//! distributed traces.
//!
//! The wrapper rewrites the statement *text* only; bind arguments and row
//! mapping flow through unchanged, so it is transparent to `sqlx::query!`
//! macro-generated queries as well as dynamically built ones.
//!
//! When there is no active span context the original query is passed through
//! untouched and the wrapper adds no overhead.

use std::borrow::Cow;

use async_stream::try_stream;
use futures_core::stream::BoxStream;
use futures_util::{TryStreamExt, future::BoxFuture};
use sqlx::{Database, Describe, Error, Execute, Executor};

use crate::{db, sql_commenter};

/// An [`sqlx::Executor`] wrapper produced by [`annotate_executor`].
#[derive(Debug)]
pub struct TraceAnnotatingExecutor<E> {
    inner: E,
}

/// Wraps `executor` so every statement executed through it is annotated with
/// the current span's `traceparent` SQL comment.
pub fn annotate_executor<'c, E>(inner: E) -> TraceAnnotatingExecutor<E>
where
    E: Executor<'c, Database = db::Db>,
{
    TraceAnnotatingExecutor { inner }
}

/// Returns the annotated statement text, or `None` when there is no active
/// span context (in which case the query should be delegated untouched).
fn annotated_sql<'q>(query: &impl Execute<'q, db::Db>) -> Option<String> {
    match sql_commenter::annotate_sql(query.sql()) {
        Cow::Borrowed(_) => None,
        Cow::Owned(sql) => Some(sql),
    }
}

/// Rebuilds a query with annotated SQL, moving out its bind arguments.
///
/// The trace context makes each statement's text unique, so the returned query
/// is marked non-persistent to avoid thrashing sqlx's per-connection prepared
/// statement cache.
fn rebuild_annotated<'q>(
    annotated: &str,
    mut query: impl Execute<'q, db::Db>,
) -> Result<sqlx::query::Query<'_, db::Db, sqlx::postgres::PgArguments>, Error> {
    let args = query
        .take_arguments()
        .map_err(Error::Encode)?
        .unwrap_or_default();
    Ok(sqlx::query_with::<db::Db, _>(annotated, args).persistent(false))
}

impl<'c, E> Executor<'c> for TraceAnnotatingExecutor<E>
where
    E: Executor<'c, Database = db::Db> + 'c,
{
    type Database = db::Db;

    fn fetch_many<'e, 'q: 'e, Q>(
        self,
        query: Q,
    ) -> BoxStream<'e, Result<sqlx::Either<<db::Db as Database>::QueryResult, db::Row>, Error>>
    where
        'c: 'e,
        Q: 'q + Execute<'q, db::Db>,
    {
        let Some(annotated) = annotated_sql(&query) else {
            return self.inner.fetch_many(query);
        };
        Box::pin(try_stream! {
            let q = rebuild_annotated(annotated.as_str(), query)?;
            let mut stream = self.inner.fetch_many(q);
            while let Some(step) = stream.try_next().await? {
                yield step;
            }
        })
    }

    fn fetch_optional<'e, 'q: 'e, Q>(
        self,
        query: Q,
    ) -> BoxFuture<'e, Result<Option<db::Row>, Error>>
    where
        'c: 'e,
        Q: 'q + Execute<'q, db::Db>,
    {
        let Some(annotated) = annotated_sql(&query) else {
            return self.inner.fetch_optional(query);
        };
        Box::pin(async move {
            let q = rebuild_annotated(annotated.as_str(), query)?;
            self.inner.fetch_optional(q).await
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<db::Db as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<db::Db as Database>::Statement<'q>, Error>>
    where
        'c: 'e,
    {
        // A prepared Statement<'q> may borrow `sql`; a locally allocated
        // annotated string could not satisfy the 'q lifetime, so preparation
        // is delegated unannotated.
        self.inner.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(self, sql: &'q str) -> BoxFuture<'e, Result<Describe<db::Db>, Error>>
    where
        'c: 'e,
    {
        // Not an execution path; delegated unannotated.
        self.inner.describe(sql)
    }
}
