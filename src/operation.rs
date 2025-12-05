//! Handle execution of database operations and transactions.

mod hooks;

use sqlx::{Acquire, PgPool, Postgres, Transaction};

/// Default return type of the derived EsRepo::begin_op().
///
/// Used as a wrapper of a [`sqlx::Transaction`] but can also cache the time at which the
/// transaction is taking place.
///
/// When `--feature sim-time` is active it will hold a time that will substitute `NOW()` in all
/// write operations.
pub struct DbOp<'c> {
    tx: Transaction<'c, Postgres>,
    now: Option<chrono::DateTime<chrono::Utc>>,
    pre_commit_hooks: Option<hooks::PreCommitHooks>,
}

impl<'c> DbOp<'c> {
    fn new(tx: Transaction<'c, Postgres>, time: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        Self {
            tx,
            now: time,
            pre_commit_hooks: Some(hooks::PreCommitHooks::new()),
        }
    }

    /// Initializes a transaction - defaults `now()` to `None` but will cache `sim_time::now()`
    /// when `--feature sim-time` is active.
    pub async fn init(pool: &PgPool) -> Result<DbOp<'static>, sqlx::Error> {
        let tx = pool.begin().await?;

        #[cfg(feature = "sim-time")]
        let time = Some(crate::prelude::sim_time::now());
        #[cfg(not(feature = "sim-time"))]
        let time = None;

        Ok(DbOp::new(tx, time))
    }

    /// Transitions to a [`DbOpWithTime`] with the given time cached.
    pub fn with_time(self, time: chrono::DateTime<chrono::Utc>) -> DbOpWithTime<'c> {
        DbOpWithTime::new(self.tx, time)
    }

    /// Transitions to a [`DbOpWithTime`] using [`chrono::Utc::now()`] to populate
    /// (unless a time was already cached from sim-time).
    pub fn with_system_time(self) -> DbOpWithTime<'c> {
        let time = if let Some(time) = self.now {
            time
        } else {
            chrono::Utc::now()
        };

        DbOpWithTime::new(self.tx, time)
    }

    /// Transitions to a [`DbOpWithTime`] using
    /// ```sql
    /// SELECT NOW()
    /// ```
    /// from the database (unless a time was already cached from sim-time).
    pub async fn with_db_time(mut self) -> Result<DbOpWithTime<'c>, sqlx::Error> {
        let time = if let Some(time) = self.now {
            time
        } else {
            let res = sqlx::query!("SELECT NOW()")
                .fetch_one(&mut *self.tx)
                .await?;
            res.now.expect("could not fetch now")
        };

        Ok(DbOpWithTime::new(self.tx, time))
    }

    /// Returns the optionally cached [`chrono::DateTime`]
    pub fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    /// Begins a nested transaction.
    pub async fn begin(&mut self) -> Result<DbOp<'_>, sqlx::Error> {
        Ok(DbOp::new(self.tx.begin().await?, self.now))
    }

    /// Commits the inner transaction.
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        let pre_commit_hooks = self.pre_commit_hooks.take().expect("no hooks");
        pre_commit_hooks.execute(&mut self).await?;
        self.tx.commit().await?;
        Ok(())
    }

    /// Gets a mutable handle to the inner transaction
    pub fn tx_mut(&mut self) -> &mut Transaction<'c, Postgres> {
        &mut self.tx
    }
}

impl<'o> AtomicOperation for DbOp<'o> {
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now()
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.tx.as_executor()
    }

    fn add_pre_commit_hook<K, H, D, Fut, M>(
        &mut self,
        hook: H,
        data: impl Into<Option<D>>,
        merge: impl Into<Option<M>>,
    ) -> bool
    where
        K: 'static,
        H: FnOnce(&mut hooks::HookOperation<'_>, D) -> Fut + Send + 'static,
        D: Send + 'static,
        Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
        M: Fn(D, D) -> D + Send + 'static,
    {
        self.pre_commit_hooks
            .as_mut()
            .expect("no hooks")
            .add::<K, H, D, Fut, M>(hook, data.into(), merge.into());
        true
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

/// Equivileant of [`DbOp`] just that the time is guaranteed to be cached.
///
/// Used as a wrapper of a [`sqlx::Transaction`] with cached time of the transaction.
pub struct DbOpWithTime<'c> {
    tx: Transaction<'c, Postgres>,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'c> DbOpWithTime<'c> {
    fn new(tx: Transaction<'c, Postgres>, time: chrono::DateTime<chrono::Utc>) -> Self {
        Self { tx, now: time }
    }

    /// The cached [`chrono::DateTime`]
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
    }

    /// Begins a nested transaction.
    pub async fn begin(&mut self) -> Result<DbOpWithTime<'_>, sqlx::Error> {
        Ok(DbOpWithTime::new(self.tx.begin().await?, self.now))
    }

    /// Commits the inner transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.tx.commit().await?;
        Ok(())
    }

    /// Gets a mutable handle to the inner transaction
    pub fn tx_mut(&mut self) -> &mut Transaction<'c, Postgres> {
        &mut self.tx
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

/// Trait to signify we can make multiple consistent database roundtrips.
///
/// Its a stand in for [`&mut sqlx::Transaction<'_, DB>`](`sqlx::Transaction`).
/// The reason for having a trait is to support custom types that wrap the inner
/// transaction while providing additional functionality.
///
/// See [`DbOp`] or [`DbOpWithTime`].
pub trait AtomicOperation: Send {
    /// Function for querying when the operation is taking place - if it is cached.
    fn now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    /// Returns the [`sqlx::Executor`] implementation.
    /// The desired way to represent this would actually be as a GAT:
    /// ```rust
    /// trait AtomicOperation {
    ///     type Executor<'c>: sqlx::PgExecutor<'c>
    ///         where Self: 'c;
    ///
    ///     fn as_executor<'c>(&'c mut self) -> Self::Executor<'c>;
    /// }
    /// ```
    ///
    /// But GATs don't play well with `async_trait::async_trait` due to lifetime constraints
    /// so we return the concrete [`&mut sqlx::PgConnection`](`sqlx::PgConnection`) instead as a work around.
    ///
    /// Since this trait is generally applied to types that wrap a [`sqlx::Transaction`]
    /// there is no variance in the return type - so its fine.
    fn as_executor(&mut self) -> &mut sqlx::PgConnection;

    fn add_pre_commit_hook<K, H, D, Fut, M>(
        &mut self,
        _hook: H,
        _data: impl Into<Option<D>>,
        _merge: impl Into<Option<M>>,
    ) -> bool
    where
        K: 'static,
        H: FnOnce(&mut hooks::HookOperation<'_>, D) -> Fut + Send + 'static,
        D: Send + 'static,
        Fut: Future<Output = Result<(), sqlx::Error>> + Send + 'static,
        M: Fn(D, D) -> D + Send + 'static,
    {
        false
    }
}

impl<'c> AtomicOperation for sqlx::Transaction<'c, Postgres> {
    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        &mut *self
    }
}
