use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::handle::ClockHandle;
use crate::inner::ClockInner;
use crate::sleep::ClockSleep;

/// Time context synchronized with a database transaction.
///
/// This provides an authoritative timestamp from the database for use
/// throughout a transaction, ensuring all operations see consistent time.
///
/// For simulated clocks, the database query is skipped and simulated time is used.
///
/// # Example
///
/// ```rust,ignore
/// use es_entity_time::ClockHandle;
///
/// async fn create_entity(clock: &ClockHandle, pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
///     let mut tx = pool.begin().await?;
///     let tx_time = clock.begin_transaction_time(&mut *tx).await?;
///
///     // All operations use the same timestamp
///     let created_at = tx_time.now();
///     sqlx::query!("INSERT INTO entities (created_at) VALUES ($1)", created_at)
///         .execute(&mut *tx)
///         .await?;
///
///     tx.commit().await?;
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct TransactionTime {
    /// The authoritative time from the database (or simulated time).
    db_time: DateTime<Utc>,
    /// What the local clock said at sync time (for drift detection).
    local_at: DateTime<Utc>,
    /// Reference to clock for duration-based operations.
    clock: ClockHandle,
}

impl TransactionTime {
    /// Create a new transaction time synchronized with the database.
    ///
    /// For simulated clocks, this skips the database query and uses simulated time.
    pub async fn new(
        clock: &ClockHandle,
        conn: &mut sqlx::PgConnection,
    ) -> Result<Self, sqlx::Error> {
        // For simulated clocks, skip the DB query
        if let ClockInner::Simulated(sim) = clock.inner() {
            let sim_time = sim.now();
            return Ok(Self {
                db_time: sim_time,
                local_at: sim_time,
                clock: clock.clone(),
            });
        }

        // For real clocks, sync with database
        let local_at = clock.now();
        let db_time: DateTime<Utc> = sqlx::query_scalar("SELECT NOW()").fetch_one(conn).await?;

        Ok(Self {
            db_time,
            local_at,
            clock: clock.clone(),
        })
    }

    /// Create a transaction time without database synchronization.
    ///
    /// Uses the clock's current time directly. Useful when you don't need
    /// DB synchronization or are already in a context where time is known.
    pub fn from_clock(clock: &ClockHandle) -> Self {
        let now = clock.now();
        Self {
            db_time: now,
            local_at: now,
            clock: clock.clone(),
        }
    }

    /// Create a transaction time with a specific time value.
    ///
    /// Useful when loading time from existing data or for testing.
    pub fn from_time(clock: &ClockHandle, time: DateTime<Utc>) -> Self {
        Self {
            db_time: time,
            local_at: clock.now(),
            clock: clock.clone(),
        }
    }

    /// Get the authoritative time for this transaction.
    ///
    /// This is the time that should be used for all persistence operations
    /// within the transaction.
    #[inline]
    pub fn now(&self) -> DateTime<Utc> {
        self.db_time
    }

    /// Get the drift between local clock and database clock.
    ///
    /// Positive duration means local clock is behind DB.
    /// Negative duration means local clock is ahead of DB.
    pub fn drift(&self) -> chrono::Duration {
        self.db_time - self.local_at
    }

    /// Get how long has elapsed since this transaction time was created.
    ///
    /// Uses the clock to measure elapsed time, not wall clock.
    pub fn elapsed(&self) -> chrono::Duration {
        self.clock.now() - self.local_at
    }

    /// Sleep for a duration, adjusted for clock drift.
    ///
    /// If you want to sleep until a specific DB time, use this method
    /// instead of `clock.sleep()` directly.
    pub fn sleep(&self, duration: Duration) -> ClockSleep {
        // Adjust for drift: if local is behind DB, sleep less
        let drift_ms = self.drift().num_milliseconds();
        let adjusted = if drift_ms > 0 {
            duration.saturating_sub(Duration::from_millis(drift_ms as u64))
        } else {
            duration + Duration::from_millis((-drift_ms) as u64)
        };
        self.clock.sleep(adjusted)
    }

    /// Get a reference to the underlying clock.
    pub fn clock(&self) -> &ClockHandle {
        &self.clock
    }
}

impl std::fmt::Debug for TransactionTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionTime")
            .field("db_time", &self.db_time)
            .field("drift_ms", &self.drift().num_milliseconds())
            .finish()
    }
}

impl ClockHandle {
    /// Begin a transaction time context synchronized with the database.
    ///
    /// This fetches the current time from the database and returns a
    /// [`TransactionTime`] that should be used for all operations within
    /// the transaction.
    ///
    /// For simulated clocks, the database query is skipped.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut tx = pool.begin().await?;
    /// let tx_time = clock.begin_transaction_time(&mut *tx).await?;
    ///
    /// // Use tx_time.now() for all timestamps in this transaction
    /// let created_at = tx_time.now();
    /// ```
    pub async fn begin_transaction_time(
        &self,
        conn: &mut sqlx::PgConnection,
    ) -> Result<TransactionTime, sqlx::Error> {
        TransactionTime::new(self, conn).await
    }

    /// Create a transaction time without database synchronization.
    ///
    /// Uses the clock's current time directly.
    pub fn transaction_time(&self) -> TransactionTime {
        TransactionTime::from_clock(self)
    }
}
