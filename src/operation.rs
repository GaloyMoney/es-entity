use sqlx::{Acquire, PgPool, Postgres, Transaction};

pub struct DbOp<'t> {
    tx: Transaction<'t, Postgres>,
    now: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'t> DbOp<'t> {
    fn new(tx: Transaction<'t, Postgres>, time: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        Self { tx, now: time }
    }

    pub async fn init(pool: &PgPool) -> Result<Self, sqlx::Error> {
        let tx = pool.begin().await?;

        #[cfg(feature = "sim-time")]
        let time = Some(sim_time::now());
        #[cfg(not(feature = "sim-time"))]
        let time = None;

        Ok(Self::new(tx, time))
    }

    pub fn with_time(self, time: chrono::DateTime<chrono::Utc>) -> DbOpWithTime<'t> {
        DbOpWithTime::new(self.tx, time)
    }

    pub fn with_system_time(self) -> DbOpWithTime<'t> {
        #[cfg(feature = "sim-time")]
        let time = sim_time::now();
        #[cfg(not(feature = "sim-time"))]
        let time = chrono::Utc::now();

        DbOpWithTime::new(self.tx, time)
    }

    pub async fn with_db_time(mut self) -> Result<DbOpWithTime<'t>, sqlx::Error> {
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

impl<'t> crate::traits::AtomicOperation for DbOp<'t> {
    type Executor<'a>
        = &'a mut sqlx::PgConnection
    where
        Self: 'a;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now()
    }

    fn as_executor(&mut self) -> Self::Executor<'_> {
        self.tx.as_executor()
    }
}

impl<'t> From<Transaction<'t, Postgres>> for DbOp<'t> {
    fn from(tx: Transaction<'t, Postgres>) -> Self {
        Self::new(tx, None)
    }
}

impl<'t> From<DbOp<'t>> for Transaction<'t, Postgres> {
    fn from(op: DbOp<'t>) -> Self {
        op.tx
    }
}

pub struct DbOpWithTime<'t> {
    tx: Transaction<'t, Postgres>,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'t> DbOpWithTime<'t> {
    fn new(tx: Transaction<'t, Postgres>, time: chrono::DateTime<chrono::Utc>) -> Self {
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

impl<'t> crate::traits::AtomicOperation for DbOpWithTime<'t> {
    type Executor<'a>
        = &'a mut sqlx::PgConnection
    where
        Self: 'a;

    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(self.now())
    }

    fn as_executor(&mut self) -> Self::Executor<'_> {
        self.tx.as_executor()
    }
}

impl<'t> From<DbOpWithTime<'t>> for Transaction<'t, Postgres> {
    fn from(op: DbOpWithTime<'t>) -> Self {
        op.tx
    }
}
