//! Savepoint-scoped operations for per-item isolation inside a single transaction.

use sqlx::{Acquire, Transaction};

use crate::{clock::ClockHandle, db};

use super::{AtomicOperation, hooks};

/// The buffer a released [`SavepointOp`]'s staged hooks fold into.
///
/// A top-level savepoint (opened via [`DbOp::begin_savepoint`] or
/// [`HookOperation::begin_savepoint`]) folds into that operation's own
/// `Option<CommitHooks>`. Both operations share this exact representation:
/// `DbOp`'s is `Some` for its whole lifetime (`None` only while a `commit()` is
/// actively draining it, which can't overlap with an open savepoint borrowing
/// the same `DbOp`); `HookOperation`'s is `Some` while a real commit pass is
/// staging through it and `None` on the [`force_execute_pre_commit`] path,
/// where there is no pass to fold into — see [`supports_hooks`](Self::supports_hooks).
///
/// A nested savepoint (opened via [`SavepointOp::begin_savepoint`]) folds into
/// its parent `SavepointOp`'s `staged` buffer instead, so releasing an N-deep
/// chain folds hooks inward one level at a time until they reach the root.
///
/// [`HookOperation::begin_savepoint`]: super::hooks::HookOperation::begin_savepoint
/// [`force_execute_pre_commit`]: hooks::CommitHook::force_execute_pre_commit
pub(super) enum HookParent<'t> {
    Root(&'t mut Option<hooks::CommitHooks>),
    Nested(&'t mut hooks::CommitHooks),
}

impl HookParent<'_> {
    /// Whether this buffer can actually receive folded hooks.
    ///
    /// `false` only for a [`Root`](Self::Root) currently holding `None` — i.e.
    /// a `HookOperation` on the `force_execute_pre_commit` path, which has no
    /// commit pass to fold into. Everything else (a live `DbOp`/`HookOperation`
    /// buffer, or any `Nested` parent) always accepts hooks.
    fn supports_hooks(&self) -> bool {
        match self {
            Self::Root(hooks) => hooks.is_some(),
            Self::Nested(_) => true,
        }
    }

    /// Folds staged hooks in. `staged` is guaranteed empty when
    /// [`supports_hooks`](Self::supports_hooks) was `false` — nothing could
    /// have been added to it, since [`SavepointOp::add_commit_hook`] itself
    /// refuses in that case — so the `None` arm is a documented no-op, not a
    /// silent drop of real hook state.
    fn absorb_staged(&mut self, staged: hooks::CommitHooks) {
        match self {
            Self::Root(Some(hooks)) => hooks.absorb_staged(staged),
            Self::Root(None) => debug_assert!(
                staged.is_empty(),
                "hooks staged on a savepoint whose root doesn't support hooks"
            ),
            Self::Nested(hooks) => hooks.absorb_staged(staged),
        }
    }

    fn get_last<H: hooks::CommitHook>(&self) -> Option<&H> {
        match self {
            Self::Root(hooks) => hooks.as_ref()?.get_last::<H>(),
            Self::Nested(hooks) => hooks.get_last::<H>(),
        }
    }
}

/// An [`AtomicOperation`] scoped to a database `SAVEPOINT` inside a parent [`DbOp`]
/// or another `SavepointOp`.
///
/// Created by [`DbOp::with_savepoint`] / [`DbOp::begin_savepoint`], or — to nest
/// one savepoint inside another — [`Self::with_savepoint`] / [`Self::begin_savepoint`].
/// Statements executed through it run inside the savepoint, so they can be undone
/// with [`rollback`](Self::rollback) without poisoning — or ending — the parent
/// transaction. This is what makes a "loop over N items in one transaction, but
/// isolate each item's failure" pattern possible: one `COMMIT` (one WAL flush)
/// for the whole batch, while a failing item unwinds only its own writes.
/// Nesting extends this to per-sub-item isolation within an already-isolated
/// item, without giving up any of the outer batch's atomicity.
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
///   parent directly. For a nested savepoint the "parent" is the enclosing
///   `SavepointOp`'s own staged buffer — folding there is not yet visible to
///   *its* parent until it, too, is released.
/// - [`rollback`](Self::rollback) issues `ROLLBACK TO SAVEPOINT` and drops the
///   staged hooks. A rolled-back item therefore contributes zero hook state to
///   match its zero database state — no phantom event publishes, no inserts
///   referencing rows that no longer exist.
///
/// No hook's [`pre_commit`] / [`post_commit`] ever runs at savepoint boundaries,
/// nested or not. They run once, at the root [`commit`](DbOp::commit), over the
/// final merged hook set — so `post_commit` still only fires after a durable
/// `COMMIT`, and [`on_rollback`] still only fires when the whole transaction is
/// gone.
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
    /// The enclosing operation's hook buffer — a `DbOp`'s own buffer for a
    /// top-level savepoint, or a parent `SavepointOp`'s `staged` buffer for a
    /// nested one. Held as a `&mut` to a *disjoint* field of that operation
    /// (the nested transaction borrows its `tx` field), so this savepoint can
    /// fold into it without the enclosing hooks ever leaving their owner.
    parent_hooks: HookParent<'t>,
}

impl<'t> SavepointOp<'t> {
    /// `acquire` is generic so this serves every concrete operation that can
    /// nest a `SAVEPOINT`: a `&mut Transaction` (from `DbOp` or another
    /// `SavepointOp`) or a bare `&mut db::Connection` (from a
    /// [`HookOperation`](super::hooks::HookOperation), which holds the raw
    /// connection rather than a `Transaction` wrapper). sqlx tracks nesting
    /// depth on the connection itself, so either one correctly issues
    /// `SAVEPOINT` rather than `BEGIN` given we're already inside a transaction.
    pub(super) async fn begin<A>(
        acquire: A,
        clock: ClockHandle,
        now: Option<chrono::DateTime<chrono::Utc>>,
        parent_hooks: HookParent<'t>,
    ) -> Result<Self, sqlx::Error>
    where
        A: Acquire<'t, Database = db::Db>,
    {
        Ok(Self {
            tx: acquire.begin().await?,
            clock,
            now,
            staged: hooks::CommitHooks::new(),
            parent_hooks,
        })
    }

    /// Releases the savepoint, keeping this scope's work.
    ///
    /// Issues `RELEASE SAVEPOINT`, then folds the staged commit hooks into the
    /// enclosing operation's buffer via the normal registration/merge path — so
    /// a mergeable hook type accumulates across savepoints exactly as it would
    /// have on the parent, and a non-mergeable one lands at its own position in
    /// release order. For a nested savepoint this is its parent `SavepointOp`'s
    /// staged buffer, not necessarily the root `DbOp` — a further `release` (or
    /// `rollback`) up the chain is what makes the fold visible further out.
    ///
    /// If the `RELEASE` itself fails the staged hooks are dropped and the error
    /// is returned: the enclosing transaction is in an indeterminate state and
    /// the caller must abandon it rather than commit or release further.
    pub async fn release(self) -> Result<(), sqlx::Error> {
        let Self {
            tx,
            staged,
            mut parent_hooks,
            ..
        } = self;
        tx.commit().await?;
        parent_hooks.absorb_staged(staged);
        Ok(())
    }

    /// Rolls back to the savepoint, discarding this scope's work.
    ///
    /// Issues `ROLLBACK TO SAVEPOINT` and drops the staged commit hooks. The
    /// enclosing operation stays alive and usable — including after an error
    /// that would otherwise have poisoned it.
    ///
    /// Dropping a `SavepointOp` without calling either `release` or `rollback`
    /// has the same database effect (sqlx queues the rollback on the
    /// connection) and likewise discards the staged hooks.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.tx.rollback().await
    }

    /// Runs `f` inside a `SAVEPOINT` nested within this one — see
    /// [`DbOp::with_savepoint`] for the full contract. Identical in behavior,
    /// just one level deeper: releasing the inner savepoint folds its staged
    /// hooks into *this* savepoint's staged buffer rather than straight into
    /// the root `DbOp`.
    pub async fn with_savepoint<T, E, F>(&mut self, f: F) -> Result<Result<T, E>, sqlx::Error>
    where
        F: AsyncFnOnce(&mut SavepointOp<'_>) -> Result<T, E>,
    {
        let mut op = self.begin_savepoint().await?;
        match f(&mut op).await {
            Ok(value) => {
                op.release().await?;
                Ok(Ok(value))
            }
            Err(error) => {
                op.rollback().await?;
                Ok(Err(error))
            }
        }
    }

    /// Begins a `SAVEPOINT` scope nested within this one, explicitly — see
    /// [`DbOp::begin_savepoint`]. Must be finished with
    /// [`release`](Self::release) or [`rollback`](Self::rollback); dropping it
    /// rolls back.
    pub async fn begin_savepoint(&mut self) -> Result<SavepointOp<'_>, sqlx::Error> {
        SavepointOp::begin(
            &mut self.tx,
            self.clock.clone(),
            self.now,
            HookParent::Nested(&mut self.staged),
        )
        .await
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

    /// Refuses, like any other operation, when the enclosing chain ultimately
    /// has nowhere to fold hooks — see [`HookParent::supports_hooks`].
    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        if !self.parent_hooks.supports_hooks() {
            return Err(hook);
        }
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
            .or_else(|| self.parent_hooks.get_last::<H>())
    }

    fn supports_hooks(&self) -> bool {
        self.parent_hooks.supports_hooks()
    }
}
