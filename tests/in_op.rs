mod helpers;

pub trait AsExecutor<'a> {
    type Executor: sqlx::Executor<'a, Database = sqlx::Postgres>;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    fn as_executor(&'a mut self) -> Self::Executor;
}

impl<'a, 't> AsExecutor<'a> for sqlx::Transaction<'t, sqlx::Postgres> {
    type Executor = &'a mut sqlx::PgConnection;

    fn as_executor(&'a mut self) -> Self::Executor {
        &mut *self
    }
}

#[tokio::test]
async fn create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?;
    check(&mut tx).await?;
    check(&mut tx).await?;

    Ok(())
}

async fn check<OP>(op: &mut OP) -> anyhow::Result<()>
where
    OP: for<'o> AsExecutor<'o>,
{
    {
        let executor = op.as_executor();
        sqlx::query!("SELECT NOW()").fetch_all(executor).await?;
    }
    inner(op).await?;
    Ok(())
}

async fn inner<'a, 'o, OP>(op: &'a mut OP) -> anyhow::Result<()>
where
    'a: 'o,
    OP: AsExecutor<'o>,
{
    let executor = op.as_executor();
    sqlx::query!("SELECT NOW()").fetch_all(executor).await?;
    Ok(())
}
