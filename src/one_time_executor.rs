//! Type-safe wrapper to ensure one database operation per executor.

use crate::operation::AtomicOperation;

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
pub struct OneTimeExecutor<'c, E>
where
    E: sqlx::PgExecutor<'c>,
{
    executor: E,
    _phantom: std::marker::PhantomData<&'c ()>,
}

impl<'c, E> OneTimeExecutor<'c, E>
where
    E: sqlx::PgExecutor<'c>,
{
    pub fn new(executor: E) -> Self {
        OneTimeExecutor {
            executor,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Proxy call to `query.fetch_all` but guarantees the inner executor will only be used once.
    pub async fn fetch_all<'q, F, O, A>(
        self,
        query: sqlx::query::Map<'q, sqlx::Postgres, F, A>,
    ) -> Result<Vec<O>, sqlx::Error>
    where
        F: FnMut(sqlx::postgres::PgRow) -> Result<O, sqlx::Error> + Send,
        O: Send + Unpin,
        A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
    {
        query.fetch_all(self.executor).await
    }

    /// Proxy call to `query.fetch_optional` but guarantees the inner executor will only be used once.
    pub async fn fetch_optional<'q, F, O, A>(
        self,
        query: sqlx::query::Map<'q, sqlx::Postgres, F, A>,
    ) -> Result<Option<O>, sqlx::Error>
    where
        F: FnMut(sqlx::postgres::PgRow) -> Result<O, sqlx::Error> + Send,
        O: Send + Unpin,
        A: 'q + Send + sqlx::IntoArguments<'q, sqlx::Postgres>,
    {
        query.fetch_optional(self.executor).await
    }
}

/// Marker trait for [`IntoOneTimeExecutorAt<'a> + 'a`](`IntoOneTimeExecutorAt`). Do not implement directly.
///
/// Used as sugar to avoid writing:
/// ```rust,ignore
/// fn some_query<'a>(op: impl IntoOnetOneExecutorAt<'a> + 'a)
/// ```
/// Instead we can shorten the signature by using elision:
/// ```rust,ignore
/// fn some_query(op: impl IntoOnetOneExecutor<'_>)
/// ```
pub trait IntoOneTimeExecutor<'c>: IntoOneTimeExecutorAt<'c> + 'c {}
impl<'c, T> IntoOneTimeExecutor<'c> for T where T: IntoOneTimeExecutorAt<'c> + 'c {}

/// A trait to signify that we can use an argument for 1 round trip to the database
///
/// Auto implemented on all [`&mut AtomicOperation`](`AtomicOperation`) types and
/// [`&sqlx::PgPool`](`sqlx::PgPool`).
pub trait IntoOneTimeExecutorAt<'c> {
    /// The concrete executor type.
    type Executor: sqlx::PgExecutor<'c>;

    /// Transforms into a [`OneTimeExecutor`] which can be used to execute a round trip.
    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c;
}

impl<'c> IntoOneTimeExecutorAt<'c> for &sqlx::PgPool {
    type Executor = &'c sqlx::PgPool;

    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c,
    {
        OneTimeExecutor::new(self)
    }
}

impl<'c, O> IntoOneTimeExecutorAt<'c> for &mut O
where
    O: AtomicOperation,
{
    type Executor = &'c mut sqlx::PgConnection;

    fn into_executor(self) -> OneTimeExecutor<'c, Self::Executor>
    where
        Self: 'c,
    {
        OneTimeExecutor::new(self.as_executor())
    }
}
