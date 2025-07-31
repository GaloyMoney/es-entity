mod helpers;

use sqlx::{Acquire, PgConnection, PgPool, Postgres, Transaction};

#[derive(Debug)]
pub struct DbOpWithSimTime<'t> {
    tx: Transaction<'t, Postgres>,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'t> DbOpWithSimTime<'t> {
    pub fn new(tx: Transaction<'t, Postgres>, time: chrono::DateTime<chrono::Utc>) -> Self {
        Self { tx, now: time }
    }
    pub async fn begin(&mut self) -> Result<DbOpWithSimTime, sqlx::Error> {
        let child_tx = self.tx.begin().await?;
        Ok(DbOpWithSimTime::new(child_tx, self.now))
    }

    /// Commits this transaction
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await
    }

    /// Rolls back this transaction
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.tx.rollback().await
    }
}

trait EsEntityOperation<'a> {
    type Executor: sqlx::Executor<'a, Database = sqlx::Postgres>;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    fn as_executor(self) -> Self::Executor;
}

impl<'a, 't> EsEntityOperation<'a> for &'a mut Transaction<'t, sqlx::Postgres> {
    type Executor = &'a mut PgConnection;

    fn as_executor(self) -> Self::Executor {
        &mut *self
    }
}

impl<'a> EsEntityOperation<'a> for &'a PgPool {
    type Executor = Self;

    fn as_executor(self) -> Self::Executor {
        self
    }
}

impl<'a, 't> EsEntityOperation<'a> for &'a mut DbOpWithSimTime<'t> {
    type Executor = &'a mut PgConnection;

    fn as_executor(self) -> Self::Executor {
        &mut *self.tx
    }
}

#[tokio::test]
async fn create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    execute(&pool).await?;
    let mut tx = pool.begin().await?;
    execute(&mut tx).await?;

    let tx = pool.begin().await?;
    let mut op = DbOpWithSimTime::new(tx, chrono::Utc::now());
    execute(&mut op).await?;
    let mut child = op.begin().await?;
    execute(&mut child).await?;
    Ok(())
}

async fn execute(op: impl EsEntityOperation<'_>) -> anyhow::Result<()> {
    dbg!("{}", op.now());
    sqlx::query!("SELECT NOW()")
        .fetch_one(op.as_executor())
        .await?;
    Ok(())
}
