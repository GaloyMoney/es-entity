pub trait AtomicOperation: Send {
    fn as_executor(&mut self) -> &mut sqlx::PgConnection;
}

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
}

pub trait IntoOneTimeExecutor<'c, E>
where
    E: sqlx::PgExecutor<'c>,
{
    fn into_executor(self) -> OneTimeExecutor<'c, E>
    where
        Self: 'c;
}

impl<'c, E> IntoOneTimeExecutor<'c, E> for E
where
    E: sqlx::PgExecutor<'c>,
{
    fn into_executor(self) -> OneTimeExecutor<'c, E>
    where
        Self: 'c,
    {
        OneTimeExecutor {
            executor: self,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<'c, O> IntoOneTimeExecutor<'c, &'c mut sqlx::PgConnection> for &mut O
where
    O: AtomicOperation,
{
    fn into_executor(self) -> OneTimeExecutor<'c, &'c mut sqlx::PgConnection>
    where
        Self: 'c,
    {
        OneTimeExecutor {
            executor: self.as_executor(),
            _phantom: std::marker::PhantomData,
        }
    }
}
