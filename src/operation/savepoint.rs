//! Savepoint-scoped operations for per-item isolation inside a single transaction.

use sqlx::{Acquire, Transaction};

use crate::{clock::ClockHandle, db};

use super::{AtomicOperation, hooks};

/// An [`AtomicOperation`] scoped to a database `SAVEPOINT` inside a parent [`DbOp`].
///
/// Created by [`DbOp::with_savepoint`] / [`DbOp::begin_savepoint`]. Statements
/// executed through it run inside the savepoint, so they can be undone with
/// [`rollback`](Self::rollback) without poisoning — or ending — the parent
/// transaction. This is what makes a "loop over N items in one transaction, but
/// isolate each item's failure" pattern possible: one `COMMIT` (one WAL flush)
/// for the whole batch, while a failing item unwinds only its own writes.
///
/// # Hooks are staged, not executed
///
/// Commit hooks registered on a `SavepointOp` — including the ones repositories
/// register internally, e.g. via `post_persist_hook` — are **staged** in a
/// private buffer rather than added to the parent operation:
///
/// - [`release`](Self::release) issues `RELEASE SAVEPOINT` and then folds the
///   staged hooks into the parent's buffer through the ordinary
///   registration/merge path, exactly as if they had been registered on the
///   parent directly.
/// - [`rollback`](Self::rollback) issues `ROLLBACK TO SAVEPOINT` and drops the
///   staged hooks. A rolled-back item therefore contributes zero hook state to
///   match its zero database state — no phantom event publishes, no inserts
///   referencing rows that no longer exist.
///
/// No hook's [`pre_commit`] / [`post_commit`] ever runs at savepoint boundaries.
/// They run once, at the parent's [`commit`](DbOp::commit), over the final merged
/// hook set — so `post_commit` still only fires after a durable `COMMIT`, and
/// [`on_rollback`] still only fires when the whole transaction is gone.
///
/// [`DbOp`]: super::DbOp
/// [`DbOp::with_savepoint`]: super::DbOp::with_savepoint
/// [`DbOp::begin_savepoint`]: super::DbOp::begin_savepoint
/// [`pre_commit`]: hooks::CommitHook::pre_commit
/// [`post_commit`]: hooks::CommitHook::post_commit
/// [`on_rollback`]: hooks::CommitHook::on_rollback
pub struct SavepointOp<'t> {
    tx: Transaction<'t, db::Db>,
    clock: ClockHandle,
    now: Option<chrono::DateTime<chrono::Utc>>,
    /// Hooks registered while the savepoint is open. Folded into
    /// `parent_hooks` on release, dropped on rollback.
    staged: hooks::CommitHooks,
    /// The parent operation's hook buffer. Held as a `&mut` to a *disjoint*
    /// field of the parent (the nested transaction borrows its `tx` field), so
    /// the savepoint can fold into it without the parent's hooks ever leaving
    /// the parent.
    parent_hooks: &'t mut Option<hooks::CommitHooks>,
}

impl<'t> SavepointOp<'t> {
    pub(super) async fn begin(
        tx: &'t mut Transaction<'_, db::Db>,
        clock: ClockHandle,
        now: Option<chrono::DateTime<chrono::Utc>>,
        parent_hooks: &'t mut Option<hooks::CommitHooks>,
    ) -> Result<Self, sqlx::Error> {
        Ok(Self {
            tx: tx.begin().await?,
            clock,
            now,
            staged: hooks::CommitHooks::new(),
            parent_hooks,
        })
    }

    /// Releases the savepoint, keeping this scope's work.
    ///
    /// Issues `RELEASE SAVEPOINT`, then folds the staged commit hooks into the
    /// parent operation's buffer via the normal registration/merge path — so a
    /// mergeable hook type accumulates across savepoints exactly as it would
    /// have on the parent, and a non-mergeable one lands at its own position in
    /// release order.
    ///
    /// If the `RELEASE` itself fails the staged hooks are dropped and the error
    /// is returned: the parent transaction is in an indeterminate state and the
    /// caller must abandon it rather than commit.
    pub async fn release(self) -> Result<(), sqlx::Error> {
        let Self {
            tx,
            staged,
            parent_hooks,
            ..
        } = self;
        tx.commit().await?;
        parent_hooks
            .as_mut()
            .expect("no hooks")
            .absorb_staged(staged);
        Ok(())
    }

    /// Rolls back to the savepoint, discarding this scope's work.
    ///
    /// Issues `ROLLBACK TO SAVEPOINT` and drops the staged commit hooks. The
    /// parent transaction stays alive and usable — including after an error
    /// that would otherwise have poisoned it.
    ///
    /// Dropping a `SavepointOp` without calling either `release` or `rollback`
    /// has the same database effect (sqlx queues the rollback on the
    /// connection) and likewise discards the staged hooks.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.tx.rollback().await
    }
}

impl AtomicOperation for SavepointOp<'_> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn clock(&self) -> &ClockHandle {
        &self.clock
    }

    fn connection(&mut self) -> &mut db::Connection {
        self.tx.connection()
    }

    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        self.staged.add(hook);
        Ok(())
    }

    /// Reads the staged buffer first, falling back to the parent's.
    ///
    /// While a savepoint is open the two are not yet merged, so a hook type
    /// present in *both* reports only the staged instance here — they become
    /// one hook when the savepoint is released.
    fn commit_hook<H: hooks::CommitHook>(&self) -> Option<&H> {
        self.staged
            .get_last::<H>()
            .or_else(|| self.parent_hooks.as_ref()?.get_last::<H>())
    }

    fn supports_hooks(&self) -> bool {
        true
    }
}
