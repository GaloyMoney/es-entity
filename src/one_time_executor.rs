//! Type-safe wrapper to ensure one database operation per executor.
//!
//! [`OneTimeExecutor`] also implements [`sqlx::Executor`]: every statement
//! executed through it is annotated with the active span's `traceparent` as a
//! trailing SQL comment (see [`crate::sql_commenter`]). This is what allows
//! statements observed server-side (`pg_stat_activity`, Postgres logs) to be
//! matched to distributed traces.
//!
//! The annotation rewrites the statement *text* only; bind arguments and row
//! mapping flow through unchanged, so it is transparent to `sqlx::query!`
//! macro-generated queries as well as dynamically built ones.
//!
//! # Trade-off: annotated statements bypass the prepared statement cache
//!
//! The trace context makes each annotated statement's text unique, so it can
//! never match sqlx's per-connection prepared statement cache. Annotated
//! statements are therefore executed with `persistent(false)` (the unnamed
//! statement) — bypassing the cache rather than thrashing it with single-use
//! entries. The cost is a server-side parse + plan per annotated execution.
//! Annotation only happens for *sampled* spans (see
//! [`crate::sql_commenter::current_traceparent`]), so un-sampled traffic keeps
//! full prepared-statement reuse.
//!
//! When there is no sampled span context the original query is passed through
//! untouched and the executor adds no overhead.

use async_stream::try_stream;
use futures_core::stream::BoxStream;
use futures_util::{TryStreamExt, future::BoxFuture};
use sqlx::{AssertSqlSafe, Database, Describe, Error, Execute, Executor, SqlStr};

use std::borrow::Cow;

use crate::{db, operation::AtomicOperation, sql_commenter};

/// A struct that owns an [`sqlx::Executor`].
///
/// Calling one of the `fetch_` `fn`s will consume it
/// thus garuanteeing a 1 time usage.
///
/// It is not used directly but passed via the [`IntoOneTimeExecutor`] trait.
///
/// In order to make the consumption of the executor work we have to pass the query to the
/// executor:
/// ```rust
/// async fn query(ex: impl es_entity::IntoOneTimeExecutor<'_>) -> Result<(), sqlx::Error> {
///     ex.into_executor().fetch_optional(
///         sqlx::query!(
///             "SELECT NOW()"
///         )
///     ).await?;
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct OneTimeExecutor<'c, E>
where
    E: sqlx::Executor<'c, Database = db::Db> + 'c,
{
    now: Option<chrono::DateTime<chrono::Utc>>,
    executor: E,
    _phantom: std::marker::PhantomData<&'c ()>,
}

impl<'c, E> OneTimeExecutor<'c, E>
where
    E: sqlx::Executor<'c, Database = db::Db> + 'c,
{
    pub(crate) fn new(executor: E, now: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        OneTimeExecutor {
            executor,
            now,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    /// Proxy call to `query.fetch_one` but guarantees the inner executor will only be used once.
    pub async fn fetch_one<'q, F, O, A>(
        self,
        query: sqlx::query::Map<'q, db::Db, F, A>,
    ) -> Result<O, sqlx::Error>
    where
        F: FnMut(db::Row) -> Result<O, sqlx::Error> + Send,
        O: Send + Unpin,
        A: 'q + Send + sqlx::IntoArguments<db::Db>,
    {
        query.fetch_one(self).await
    }

    /// Proxy call to `query.fetch_all` but guarantees the inner executor will only be used once.
    pub async fn fetch_all<'q, F, O, A>(
        self,
        query: sqlx::query::Map<'q, db::Db, F, A>,
    ) -> Result<Vec<O>, sqlx::Error>
    where
        F: FnMut(db::Row) -> Result<O, sqlx::Error> + Send,
        O: Send + Unpin,
        A: 'q + Send + sqlx::IntoArguments<db::Db>,
    {
        query.fetch_all(self).await
    }

    /// Proxy call to `query.fetch_optional` but guarantees the inner executor will only be used once.
    pub async fn fetch_optional<'q, F, O, A>(
        self,
        query: sqlx::query::Map<'q, db::Db, F, A>,
    ) -> Result<Option<O>, sqlx::Error>
    where
        F: FnMut(db::Row) -> Result<O, sqlx::Error> + Send,
        O: Send + Unpin,
        A: 'q + Send + sqlx::IntoArguments<db::Db>,
    {
        query.fetch_optional(self).await
    }
}

/// Whether statements should currently be annotated, i.e. there is a sampled
/// span context. When `false` the query is delegated untouched.
///
/// `Execute::sql` consumes the query in sqlx 0.9, so this must be decided
/// before looking at the statement text.
fn annotation_active() -> bool {
    matches!(sql_commenter::annotate_sql(""), Cow::Owned(_))
}

/// Rebuilds a query with annotated SQL, moving out its bind arguments.
///
/// The trace context makes each statement's text unique, so the returned query
/// is marked non-persistent: it can never hit sqlx's per-connection prepared
/// statement cache, and caching it would evict useful entries. The trade-off
/// is a server-side parse + plan per annotated execution.
fn rebuild_annotated<'q>(
    mut query: impl Execute<'q, db::Db>,
) -> Result<sqlx::query::Query<'static, db::Db, db::Arguments>, Error> {
    let args = query
        .take_arguments()
        .map_err(Error::Encode)?
        .unwrap_or_default();
    let sql = query.sql();
    // The text originates from an already-vetted query; only the trace
    // comment (hex ids) is appended.
    let annotated = sql_commenter::annotate_sql(sql.as_str()).into_owned();
    Ok(sqlx::query_with::<db::Db, _>(AssertSqlSafe(annotated), args).persistent(false))
}

impl<'c, E> Executor<'c> for OneTimeExecutor<'c, E>
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
        if !annotation_active() {
            return self.executor.fetch_many(query);
        }
        Box::pin(try_stream! {
            let q = rebuild_annotated(query)?;
            let mut stream = self.executor.fetch_many(q);
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
        if !annotation_active() {
            return self.executor.fetch_optional(query);
        }
        Box::pin(async move {
            let q = rebuild_annotated(query)?;
            self.executor.fetch_optional(q).await
        })
    }

    fn prepare_with<'e>(
        self,
        sql: SqlStr,
        parameters: &'e [<db::Db as Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<db::Db as Database>::Statement, Error>>
    where
        'c: 'e,
    {
        // Explicit preparation is not an execution path; delegated unannotated.
        self.executor.prepare_with(sql, parameters)
    }

    fn describe<'e>(self, sql: SqlStr) -> BoxFuture<'e, Result<Describe<db::Db>, Error>>
    where
        'c: 'e,
    {
        // Not an execution path; delegated unannotated.
        self.executor.describe(sql)
    }
}

/// Marker trait for [`IntoOneTimeExecutorAt<'a> + 'a`](`IntoOneTimeExecutorAt`). Do not implement directly.
///
/// Used as sugar to avoid writing:
/// ```rust,ignore
/// fn some_query<'a>(op: impl IntoOneTimeExecutorAt<'a> + 'a)
/// ```
/// Instead we can shorten the signature by using elision:
/// ```rust,ignore
/// fn some_query(op: impl IntoOneTimeExecutor<'_>)
/// ```
pub trait IntoOneTimeExecutor<'c>: IntoOneTimeExecutorAt<'c> + 'c {}
impl<'c, T> IntoOneTimeExecutor<'c> for T where T: IntoOneTimeExecutorAt<'c> + 'c {}

/// A trait to signify that we can use an argument for 1 round trip to the database
///
/// Auto implemented on all [`&mut AtomicOperation`](`AtomicOperation`) types and
/// [`&db::Pool`](`crate::db::Pool`).
pub trait IntoOneTimeExecutorAt<'c> {
    /// The concrete executor type.
    type Executor: sqlx::Executor<'c, Database = db::Db>;

    /// Transforms into a [`OneTimeExecutor`] which can be used to execute a round trip.
    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c;
}

impl<'c, E> IntoOneTimeExecutorAt<'c> for OneTimeExecutor<'c, E>
where
    E: sqlx::Executor<'c, Database = db::Db> + 'c,
{
    type Executor = E;

    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c,
    {
        self
    }
}

impl<'c> IntoOneTimeExecutorAt<'c> for &db::Pool {
    type Executor = &'c db::Pool;

    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c,
    {
        OneTimeExecutor::new(self, None)
    }
}

impl<'c, O> IntoOneTimeExecutorAt<'c> for &mut O
where
    O: AtomicOperation + ?Sized,
{
    type Executor = &'c mut db::Connection;

    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c,
    {
        let now = self.maybe_now();
        OneTimeExecutor::new(self.connection(), now)
    }
}
