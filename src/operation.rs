//! Handle execution of database operations and transactions.

use sqlx::{Acquire, PgPool, Postgres, Transaction};

pub struct DbOp<'c> {
    tx: Transaction<'c, Postgres>,
    now: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'c> DbOp<'c> {
    fn new(tx: Transaction<'c, Postgres>, time: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        Self { tx, now: time }
    }

    pub async fn init(pool: &PgPool) -> Result<DbOp<'static>, sqlx::Error> {
        let tx = pool.begin().await?;

        #[cfg(feature = "sim-time")]
        let time = Some(sim_time::now());
        #[cfg(not(feature = "sim-time"))]
        let time = None;

        Ok(DbOp::new(tx, time))
    }

    pub fn with_time(self, time: chrono::DateTime<chrono::Utc>) -> DbOpWithTime<'c> {
        DbOpWithTime::new(self.tx, time)
    }

    pub fn with_system_time(self) -> DbOpWithTime<'c> {
        #[cfg(feature = "sim-time")]
        let time = sim_time::now();
        #[cfg(not(feature = "sim-time"))]
        let time = chrono::Utc::now();

        DbOpWithTime::new(self.tx, time)
    }

    pub async fn with_db_time(mut self) -> Result<DbOpWithTime<'c>, sqlx::Error> {
        #[cfg(feature = "sim-time")]
        let time = sim_time::now();
        #[cfg(not(feature = "sim-time"))]
        let time = {
            let res = sqlx::query!("SELECT NOW()")
                .fetch_one(&mut *self.tx)
                .await?;
            res.now.expect("could not fetch now")
        };

        Ok(DbOpWithTime::new(self.tx, time))
    }

    pub fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    pub async fn begin(&mut self) -> Result<DbOp<'_>, sqlx::Error> {
        Ok(DbOp::new(self.tx.begin().await?, self.now))
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await?;
        Ok(())
    }
}

impl<'o> AtomicOperation for DbOp<'o> {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now()
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.tx.as_executor()
    }
}

impl<'c> From<Transaction<'c, Postgres>> for DbOp<'c> {
    fn from(tx: Transaction<'c, Postgres>) -> Self {
        Self::new(tx, None)
    }
}

impl<'c> From<DbOp<'c>> for Transaction<'c, Postgres> {
    fn from(op: DbOp<'c>) -> Self {
        op.tx
    }
}

pub struct DbOpWithTime<'c> {
    tx: Transaction<'c, Postgres>,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'c> DbOpWithTime<'c> {
    fn new(tx: Transaction<'c, Postgres>, time: chrono::DateTime<chrono::Utc>) -> Self {
        Self { tx, now: time }
    }

    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
    }

    pub async fn begin(&mut self) -> Result<DbOpWithTime<'_>, sqlx::Error> {
        Ok(DbOpWithTime::new(self.tx.begin().await?, self.now))
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await?;
        Ok(())
    }
}

impl<'o> AtomicOperation for DbOpWithTime<'o> {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(self.now())
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.tx.as_executor()
    }
}

impl<'c> From<DbOpWithTime<'c>> for Transaction<'c, Postgres> {
    fn from(op: DbOpWithTime<'c>) -> Self {
        op.tx
    }
}

pub trait AtomicOperation: Send {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection;
}

impl<'c> AtomicOperation for sqlx::PgTransaction<'c> {
    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        &mut *self
    }
}
