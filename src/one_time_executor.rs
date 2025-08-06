//! Type-safe wrapper to ensure one database operation per executor
use crate::operation::AtomicOperation;

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

pub trait IntoOneTimeExecutor<'c>: IntoOneTimeExecutorAt<'c> + 'c {}
impl<'c, T> IntoOneTimeExecutor<'c> for T where T: IntoOneTimeExecutorAt<'c> + 'c {}

pub trait IntoOneTimeExecutorAt<'c> {
    type Executor: sqlx::PgExecutor<'c>;

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
