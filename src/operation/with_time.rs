use crate::clock::ClockHandle;

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
    ///
    /// Priority order:
    /// 1. Cached time from operation
    /// 2. Artificial clock time if the operation's clock is artificial (and hasn't transitioned)
    /// 3. Database time via `SELECT NOW()`
    pub async fn cached_or_db_time(op: &'a mut Op) -> Result<Self, sqlx::Error> {
        let now = if let Some(time) = op.maybe_now() {
            time
        } else if let Some(artificial_time) = op.clock().artificial_now() {
            artificial_time
        } else {
            let res = sqlx::query!("SELECT NOW()")
                .fetch_one(op.as_executor())
                .await?;
            res.now.expect("could not fetch now")
        };
        Ok(Self { inner: op, now })
    }

    /// Wraps with a specific time (uses existing if present).
    pub fn cached_or_time(op: &'a mut Op, time: chrono::DateTime<chrono::Utc>) -> Self {
        let now = op.maybe_now().unwrap_or(time);
        Self { inner: op, now }
    }

    /// Wraps using system time (uses existing if present).
    ///
    /// Uses cached time if present, otherwise uses the operation's clock.
    pub fn cached_or_clock_time(op: &'a mut Op) -> Self {
        let now = op.maybe_now().unwrap_or_else(|| op.clock().now());
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

    fn clock(&self) -> &ClockHandle {
        self.inner.clock()
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.inner.as_executor()
    }

    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        self.inner.add_commit_hook(hook)
    }
}
