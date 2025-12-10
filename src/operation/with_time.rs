use super::{AtomicOperation, hooks};

pub trait AtomicOperationWithTime: AtomicOperation {
    fn now(&self) -> chrono::DateTime<chrono::Utc>;
}

/// Wrapper that guarantees time is available, borrowing the underlying operation.
pub struct OpWithTime<'a, Op: AtomicOperation + ?Sized> {
    inner: &'a mut Op,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'a, Op: AtomicOperation + ?Sized> AtomicOperationWithTime for OpWithTime<'a, Op> {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
    }
}

impl<'a, Op: AtomicOperation + ?Sized> OpWithTime<'a, Op> {
    /// Wraps an operation, using existing time if present, otherwise fetching from DB.
    pub async fn cached_or_db_time(op: &'a mut Op) -> Result<Self, sqlx::Error> {
        let now = if let Some(time) = op.maybe_now() {
            time
        } else {
            #[cfg(feature = "sim-time")]
            {
                crate::prelude::sim_time::now()
            }
            #[cfg(not(feature = "sim-time"))]
            {
                let res = sqlx::query!("SELECT NOW()")
                    .fetch_one(op.as_executor())
                    .await?;
                res.now.expect("could not fetch now")
            }
        };
        Ok(Self { inner: op, now })
    }

    /// Wraps with a specific time (uses existing if present).
    pub fn cached_or_time(op: &'a mut Op, time: chrono::DateTime<chrono::Utc>) -> Self {
        let now = op.maybe_now().unwrap_or(time);
        Self { inner: op, now }
    }

    /// Wraps using system time (uses existing if present).
    pub fn cached_or_system_time(op: &'a mut Op) -> Self {
        let now = op.maybe_now().unwrap_or_else(|| {
            #[cfg(feature = "sim-time")]
            {
                crate::prelude::sim_time::now()
            }
            #[cfg(not(feature = "sim-time"))]
            {
                chrono::Utc::now()
            }
        });
        Self { inner: op, now }
    }

    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
    }
}

impl<'a, Op: AtomicOperation + ?Sized> AtomicOperation for OpWithTime<'a, Op> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(self.now)
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.inner.as_executor()
    }

    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        self.inner.add_commit_hook(hook)
    }
}
