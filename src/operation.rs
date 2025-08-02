use sqlx::{PgPool, Postgres, Transaction};

pub struct DbOp<'t> {
    tx: Transaction<'t, Postgres>,
    now: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'t> DbOp<'t> {
    pub fn new(tx: Transaction<'t, Postgres>, time: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            tx,
            now: Some(time),
        }
    }

    pub async fn init(pool: &PgPool) -> Result<Self, sqlx::Error> {
        #[cfg(feature = "sim-time")]
        let res = {
            let tx = pool.begin().await?;
            let now = sim_time::now();
            Self { tx, now: Some(now) }
        };

        #[cfg(not(feature = "sim-time"))]
        let res = {
            let tx = pool.begin().await?;
            Self { tx, now: None }
        };

        Ok(res)
    }

    pub fn tx(&mut self) -> &mut Transaction<'t, Postgres> {
        &mut self.tx
    }

    pub fn into_tx(self) -> Transaction<'t, Postgres> {
        self.tx
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await?;
        Ok(())
    }
}

impl<'a, 't> crate::traits::AtomicOperation<'a> for DbOp<'t> {
    type Executor = &'a mut sqlx::PgConnection;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn as_executor(&'a mut self) -> Self::Executor {
        self.tx.as_executor()
    }
}
