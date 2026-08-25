//! Handle execution of database operations and transactions.

pub mod hooks;
mod savepoint;
mod with_time;

use sqlx::{Acquire, Transaction};

use crate::{clock::ClockHandle, db, one_time_executor::OneTimeExecutor};

pub use savepoint::*;
pub use with_time::*;

/// Default return type of the derived EsRepo::begin_op().
///
/// Used as a wrapper of a [`sqlx::Transaction`] but can also cache the time at which the
/// transaction is taking place.
///
/// When a manual clock is provided, the transaction will automatically cache that
/// clock's time, enabling deterministic testing. This cached time will be used in all
/// time-dependent operations.
pub struct DbOp<'c> {
    tx: Transaction<'c, db::Db>,
    clock: ClockHandle,
    now: Option<chrono::DateTime<chrono::Utc>>,
    commit_hooks: Option<hooks::CommitHooks>,
}

impl<'c> DbOp<'c> {
    fn new(
        tx: Transaction<'c, db::Db>,
        clock: ClockHandle,
        time: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Self {
        Self {
            tx,
            clock,
            now: time,
            commit_hooks: Some(hooks::CommitHooks::new()),
        }
    }

    /// Initializes a transaction using the global clock.
    ///
    /// Delegates to [`init_with_clock`](Self::init_with_clock) using the global clock handle.
    pub async fn init(pool: &db::Pool) -> Result<DbOp<'static>, sqlx::Error> {
        Self::init_with_clock(pool, crate::clock::Clock::handle()).await
    }

    /// Initializes a transaction with the specified clock.
    ///
    /// If the clock is manual, its current time will be cached in the transaction.
    pub async fn init_with_clock(
        pool: &db::Pool,
        clock: &ClockHandle,
    ) -> Result<DbOp<'static>, sqlx::Error> {
        let tx = pool.begin().await?;

        // If a manual clock is provided, cache its time for consistent
        // timestamps within the transaction.
        let time = clock.manual_now();

        Ok(DbOp::new(tx, clock.clone(), time))
    }

    /// Transitions to a [`DbOpWithTime`] with the given time cached.
    pub fn with_time(self, time: chrono::DateTime<chrono::Utc>) -> DbOpWithTime<'c> {
        DbOpWithTime::new(self, time)
    }

    /// Transitions to a [`DbOpWithTime`] using the clock.
    ///
    /// Uses cached time if present, otherwise uses the clock's current time.
    pub fn with_clock_time(self) -> DbOpWithTime<'c> {
        let time = self.now.unwrap_or_else(|| self.clock.now());
        DbOpWithTime::new(self, time)
    }

    /// Transitions to a [`DbOpWithTime`] using the database time.
    ///
    /// Priority order:
    /// 1. Cached time if present
    /// 2. Manual clock time if the clock is manual
    /// 3. Database time via `SELECT NOW()`
    pub async fn with_db_time(mut self) -> Result<DbOpWithTime<'c>, sqlx::Error> {
        let time = if let Some(time) = self.now {
            time
        } else if let Some(manual_time) = self.clock.manual_now() {
            manual_time
        } else {
            db::database_now(&mut *self.tx).await?
        };

        Ok(DbOpWithTime::new(self, time))
    }

    /// Returns the optionally cached [`chrono::DateTime`]
    pub fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    /// Begins a nested transaction.
    pub async fn begin(&mut self) -> Result<DbOp<'_>, sqlx::Error> {
        Ok(DbOp::new(
            self.tx.begin().await?,
            self.clock.clone(),
            self.now,
        ))
    }

    /// Runs `f` inside a `SAVEPOINT`, keeping its work on `Ok` and undoing it on `Err`.
    ///
    /// This is the building block for processing a batch of items in **one**
    /// transaction — one `COMMIT`, one WAL flush — while still isolating each
    /// item's failure. An item that errors unwinds only its own writes and
    /// staged commit hooks; the transaction stays usable, so the loop continues
    /// and its healthy items still commit.
    ///
    /// # Two layers of `Result`
    ///
    /// - The **outer** `Err(sqlx::Error)` means the savepoint machinery itself
    ///   failed (or the error was never savepoint-recoverable, e.g. the
    ///   connection died). The parent operation is in an indeterminate state:
    ///   abandon it, don't commit.
    /// - The **inner** `Err(E)` is the item's own failure, already rolled back
    ///   cleanly. Record the outcome and keep going.
    ///
    /// If the closure fails *and* the rollback fails, the rollback error is
    /// returned as the outer `Err` and the item's error is dropped — the
    /// poisoned-transaction signal is what the caller must act on.
    ///
    /// # Collecting per-item outcomes
    ///
    /// The closure may borrow from its environment, but host-side mutations do
    /// **not** unwind with the savepoint. Return the item's verdict through
    /// `Ok`/`Err` and record it outside, where the outcome is authoritative:
    ///
    /// ```rust,ignore
    /// let mut op = DbOp::init(&pool).await?;
    /// let mut outcomes = Vec::with_capacity(items.len());
    ///
    /// for item in items {
    ///     // `?` here: infra failure — abandon the whole batch.
    ///     let res = op
    ///         .with_savepoint(async |op| self.process_in_op(op, item).await)
    ///         .await?;
    ///
    ///     outcomes.push(match res {
    ///         Ok(()) => Outcome::Complete,
    ///         Err(e) => Outcome::Retry(e),
    ///     });
    /// }
    ///
    /// op.commit().await?;
    /// ```
    ///
    /// See [`SavepointOp`] for how commit hooks are staged and folded in.
    ///
    /// Kept as an inherent method so existing call sites need no import; the
    /// behaviour lives in [`SavepointOperation::with_savepoint`], which every
    /// [`AtomicOperation`] gets.
    pub async fn with_savepoint<T, E, F>(&mut self, f: F) -> Result<Result<T, E>, sqlx::Error>
    where
        F: AsyncFnOnce(&mut SavepointOp<'_>) -> Result<T, E>,
    {
        SavepointOperation::with_savepoint(self, f).await
    }

    /// Begins a `SAVEPOINT` scope explicitly.
    ///
    /// The escape hatch for when [`with_savepoint`](Self::with_savepoint)'s
    /// closure form doesn't fit — the returned [`SavepointOp`] must be finished
    /// with [`release`](SavepointOp::release) or
    /// [`rollback`](SavepointOp::rollback). Dropping it rolls back.
    pub async fn begin_savepoint(&mut self) -> Result<SavepointOp<'_>, sqlx::Error> {
        SavepointOperation::begin_savepoint(self).await
    }

    /// Commits the inner transaction.
    ///
    /// On the failure paths the commit hooks' [`on_rollback`] runs **after** the
    /// transaction is definitively gone, so hook-side compensation never
    /// contends with the dying transaction's own locks:
    ///
    /// - A later hook's `pre_commit` fails → the transaction is rolled back
    ///   first, *then* the earlier (already-pre_committed) hooks are notified.
    /// - The `COMMIT` itself fails → the transaction is over server-side either
    ///   way, so the hooks are notified directly (their side effects must be
    ///   idempotent against a possibly-landed commit).
    ///
    /// [`on_rollback`]: hooks::CommitHook::on_rollback
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        let commit_hooks = self.commit_hooks.take().expect("no hooks");
        match commit_hooks.execute_pre(&mut self).await {
            Ok(post_hooks) => match self.tx.commit().await {
                Ok(()) => {
                    post_hooks.execute();
                    Ok(())
                }
                Err(error) => {
                    // The commit attempt is definitively over server-side (it
                    // may have landed despite the error, or aborted) — there is
                    // no rollback to issue. Fire `on_rollback` so hooks can
                    // signal; their side effects must be idempotent against a
                    // possibly-landed commit.
                    post_hooks.execute_rollback();
                    Err(error)
                }
            },
            Err((error, executed)) => {
                // A later hook's `pre_commit` failed. Roll back BEFORE
                // signalling: the rollback is awaited so it has landed
                // server-side before any `on_rollback` fires, so a hook's
                // downstream compensation never contends with this dying
                // transaction's own locks. A rollback error means the
                // connection is being torn down (which aborts the transaction
                // anyway) — swallow it and surface the original hook error.
                let _ = self.tx.rollback().await;
                executed.execute_rollback();
                Err(error)
            }
        }
    }

    /// Gets a mutable handle to the inner transaction
    pub fn tx_mut(&mut self) -> &mut Transaction<'c, db::Db> {
        &mut self.tx
    }
}

impl<'o> AtomicOperation for DbOp<'o> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.maybe_now()
    }

    fn clock(&self) -> &ClockHandle {
        &self.clock
    }

    fn connection(&mut self) -> &mut db::Connection {
        self.tx.connection()
    }

    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        self.commit_hooks.as_mut().expect("no hooks").add(hook);
        Ok(())
    }

    fn commit_hook<H: hooks::CommitHook>(&self) -> Option<&H> {
        self.commit_hooks.as_ref()?.get_last::<H>()
    }

    fn supports_hooks(&self) -> bool {
        true
    }

    /// `tx` and `commit_hooks` are disjoint fields, so both can be borrowed
    /// mutably in one expression — the borrow split that a pair of `&mut self`
    /// accessors could not express, which is the whole reason this method
    /// returns both halves at once.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, savepoint::HookSlot<'_>) {
        (
            self.tx.connection(),
            savepoint::HookSlot(self.commit_hooks.as_mut()),
        )
    }
}

/// Equivileant of [`DbOp`] just that the time is guaranteed to be cached.
///
/// Used as a wrapper of a [`sqlx::Transaction`] with cached time of the transaction.
pub struct DbOpWithTime<'c> {
    inner: DbOp<'c>,
    now: chrono::DateTime<chrono::Utc>,
}

impl<'c> DbOpWithTime<'c> {
    fn new(mut inner: DbOp<'c>, time: chrono::DateTime<chrono::Utc>) -> Self {
        inner.now = Some(time);
        Self { inner, now: time }
    }

    /// The cached [`chrono::DateTime`]
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
    }

    /// Begins a nested transaction.
    pub async fn begin(&mut self) -> Result<DbOpWithTime<'_>, sqlx::Error> {
        Ok(DbOpWithTime::new(self.inner.begin().await?, self.now))
    }

    /// Runs `f` inside a `SAVEPOINT` — see [`DbOp::with_savepoint`].
    ///
    /// The cached time is propagated, so the [`SavepointOp`] reports it from
    /// [`maybe_now`](AtomicOperation::maybe_now) and wrapping it in
    /// [`OpWithTime`] is free.
    pub async fn with_savepoint<T, E, F>(&mut self, f: F) -> Result<Result<T, E>, sqlx::Error>
    where
        F: AsyncFnOnce(&mut SavepointOp<'_>) -> Result<T, E>,
    {
        SavepointOperation::with_savepoint(self, f).await
    }

    /// Begins a `SAVEPOINT` scope explicitly — see [`DbOp::begin_savepoint`].
    pub async fn begin_savepoint(&mut self) -> Result<SavepointOp<'_>, sqlx::Error> {
        SavepointOperation::begin_savepoint(self).await
    }

    /// Commits the inner transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.inner.commit().await
    }

    /// Gets a mutable handle to the inner transaction
    pub fn tx_mut(&mut self) -> &mut Transaction<'c, db::Db> {
        self.inner.tx_mut()
    }
}

impl<'o> AtomicOperation for DbOpWithTime<'o> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(self.now())
    }

    fn clock(&self) -> &ClockHandle {
        self.inner.clock()
    }

    fn connection(&mut self) -> &mut db::Connection {
        self.inner.connection()
    }

    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        self.inner.add_commit_hook(hook)
    }

    fn commit_hook<H: hooks::CommitHook>(&self) -> Option<&H> {
        self.inner.commit_hook::<H>()
    }

    fn supports_hooks(&self) -> bool {
        self.inner.supports_hooks()
    }

    fn savepoint_parts(&mut self) -> (&mut db::Connection, savepoint::HookSlot<'_>) {
        self.inner.savepoint_parts()
    }
}

impl<'o> AtomicOperationWithTime for DbOpWithTime<'o> {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.now
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
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    /// Returns the clock handle for time operations.
    ///
    /// Default implementation returns the global clock handle.
    fn clock(&self) -> &ClockHandle {
        crate::clock::Clock::handle()
    }

    /// Returns the raw underlying connection.
    /// The desired way to represent this would actually be as a GAT:
    /// ```rust
    /// trait AtomicOperation {
    ///     type Executor<'c>: sqlx::PgExecutor<'c>
    ///         where Self: 'c;
    ///
    ///     fn connection<'c>(&'c mut self) -> Self::Executor<'c>;
    /// }
    /// ```
    ///
    /// But GATs don't play well with `async_trait::async_trait` due to lifetime constraints
    /// so we return the concrete [`&mut db::Connection`](`crate::db::Connection`) instead as a work around.
    ///
    /// Since this trait is generally applied to types that wrap a [`sqlx::Transaction`]
    /// there is no variance in the return type - so its fine.
    ///
    /// Statements executed directly on the returned connection are **not**
    /// annotated with trace context — use [`as_executor`](Self::as_executor)
    /// unless raw connection access is required.
    fn connection(&mut self) -> &mut db::Connection;

    /// Returns the [`sqlx::Executor`] implementation that statements should be
    /// executed through.
    ///
    /// The returned [`OneTimeExecutor`] annotates every statement with the
    /// current span's `traceparent` SQL comment when the `tracing-context`
    /// feature is enabled and a *sampled* span is active (see
    /// [`crate::sql_commenter`]). Otherwise statements pass through untouched.
    ///
    /// Trade-off: the trace context makes annotated statement text unique, so
    /// annotated statements bypass sqlx's per-connection prepared statement
    /// cache (`persistent(false)`) — costing a server-side parse + plan per
    /// execution. Un-annotated traffic keeps full prepared-statement reuse.
    fn as_executor(&mut self) -> OneTimeExecutor<'_, &mut db::Connection> {
        let now = self.maybe_now();
        OneTimeExecutor::new(self.connection(), now)
    }

    /// Registers a commit hook that will run pre_commit before and post_commit after the transaction commits.
    /// Returns Ok(()) if the hook was registered, Err(hook) if hooks are not supported.
    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        Err(hook)
    }

    /// Typed shared access to the currently-accumulating commit hook of type `H`,
    /// if this operation supports commit hooks and one is registered.
    /// Returns the hook a subsequent `add_commit_hook::<H>` call would merge into.
    fn commit_hook<H: hooks::CommitHook>(&self) -> Option<&H> {
        None
    }

    /// Whether this operation supports commit hooks.
    ///
    /// `true` iff [`add_commit_hook`](Self::add_commit_hook) can register a hook
    /// (i.e. the operation is backed by a [`DbOp`]-style commit-hook buffer, not
    /// a bare [`sqlx::Transaction`]). Unlike [`commit_hook`](Self::commit_hook) —
    /// whose `None` is ambiguous between "hooks unsupported" and "supported but
    /// none registered yet" — this reports support directly, with no registration
    /// attempt and no `&mut` access.
    fn supports_hooks(&self) -> bool {
        false
    }

    /// Simultaneous access to the connection **and** the commit-hook buffer a
    /// nested `SAVEPOINT` folds into when released. Implementing this is the
    /// only thing an operation must do to get the whole of
    /// [`SavepointOperation`] — `with_savepoint`, `begin_savepoint`, and
    /// arbitrary-depth nesting — for free.
    ///
    /// Returning both halves together is not a convenience: it is a
    /// requirement. A [`SavepointOp`] holds a `&mut` to the connection *and* a
    /// `&mut` to the hook buffer for its entire lifetime, and two separate
    /// `&mut self` accessors can never be live at the same time. Returning the
    /// pair lets an implementor split the borrow across its own disjoint fields
    /// — legal inside the type, impossible across a trait boundary otherwise:
    ///
    /// ```rust,ignore
    /// fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
    ///     // `tx` and `commit_hooks` are different fields, so this is fine.
    ///     (self.tx.connection(), HookSlot::root(&mut self.commit_hooks))
    /// }
    /// ```
    ///
    /// An operation that wraps another should **forward** to the inner one, so
    /// hook support is preserved:
    ///
    /// ```rust,ignore
    /// fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
    ///     self.inner.savepoint_parts()
    /// }
    /// ```
    ///
    /// An operation with no hook buffer of its own returns
    /// [`HookSlot::unsupported`] — savepoints still work at the database level,
    /// hook registration inside them refuses, and callers fall back to
    /// [`force_execute_pre_commit`](hooks::CommitHook::force_execute_pre_commit)
    /// exactly as they already do on the operation itself.
    ///
    /// There is deliberately **no default implementation**. A default could only
    /// return [`HookSlot::unsupported`], which would compile for a wrapper type
    /// that forgot to override it while silently downgrading every savepoint
    /// taken through it to "hooks unsupported" — turning a missing four-line
    /// method into a quiet behavioural change instead of a compile error.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, savepoint::HookSlot<'_>);
}

impl<'c> AtomicOperation for sqlx::Transaction<'c, db::Db> {
    fn connection(&mut self) -> &mut db::Connection {
        &mut *self
    }

    /// A bare transaction carries no commit-hook buffer, so savepoints taken
    /// through it work at the database level and refuse hook registration.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, savepoint::HookSlot<'_>) {
        (&mut *self, savepoint::HookSlot::unsupported())
    }
}
