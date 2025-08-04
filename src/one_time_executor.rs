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
}
