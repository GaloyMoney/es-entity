//! Savepoint-scoped operations for per-item isolation inside a single transaction.

use sqlx::{Acquire, Transaction};

use std::future::Future;

use crate::{clock::ClockHandle, db};

use super::{AtomicOperation, hooks};

/// The buffer a released savepoint's staged hooks fold into, or `None` when
/// there is nowhere for them to go.
///
/// Deliberately an `Option<&mut CommitHooks>` rather than an enum
/// distinguishing "root" from "nested": the distinction carried no behavioural
/// difference, and encoding it invited the bug where a *nested* parent was
/// assumed to accept hooks regardless of whether the chain above it did. With
/// one representation, capability is a property of the buffer itself and
/// propagates down a nesting chain by construction.
///
/// `None` arises from a bare [`sqlx::Transaction`] or any implementor that opts
/// out via [`HookSlot::unsupported`]; from a `DbOp`/`HookOperation` whose own
/// buffer is `None` (the [`force_execute_pre_commit`] path, which has no commit
/// pass to fold into); and — transitively — from any savepoint nested inside one
/// of those.
///
/// [`force_execute_pre_commit`]: hooks::CommitHook::force_execute_pre_commit
pub(super) type HookParent<'t> = Option<&'t mut hooks::CommitHooks>;

/// Folds `staged` into `parent`.
///
/// Errors — rather than silently dropping — if hooks were staged against a
/// parent that cannot receive them. With capability propagated correctly this
/// is unreachable, so it is defence in depth: it converts a future regression
/// from silent hook loss (`pre_commit`/`post_commit` never running for work the
/// caller was told had been registered) into a loud failure, in release builds
/// as well as debug. Mirrors how the crate already reports the impossible
/// `runs_after` cycle.
fn absorb_staged(
    parent: &mut HookParent<'_>,
    staged: hooks::CommitHooks,
) -> Result<(), sqlx::Error> {
    match parent {
        Some(hooks) => {
            hooks.absorb_staged(staged);
            Ok(())
        }
        None if staged.is_empty() => Ok(()),
        None => Err(sqlx::Error::Protocol(
            "commit hooks were staged on a savepoint whose enclosing operation \
             cannot receive them — they would never run. This is a bug in the \
             savepoint hook-capability propagation."
                .to_string(),
        )),
    }
}

/// Where a released [`SavepointOp`]'s staged commit hooks fold into.
///
/// Returned as the second half of
/// [`AtomicOperation::savepoint_parts`](super::AtomicOperation::savepoint_parts),
/// paired with the connection the savepoint runs on. It is deliberately opaque:
/// the only things an implementor outside this crate can do with one are
/// *forward* a slot obtained from an operation it wraps, or declare that it has
/// no hook buffer via [`unsupported`](Self::unsupported).
///
/// The pairing is the point. A savepoint needs a `&mut` to the connection **and**
/// a `&mut` to the hook buffer, held simultaneously for its whole lifetime. Two
/// separate `&mut self` accessors could never hand out both at once; returning
/// them together lets the implementor split the borrow across its own disjoint
/// fields, where the compiler permits it, and pass the result across the trait
/// boundary already split.
pub struct HookSlot<'t>(pub(super) HookParent<'t>);

impl HookSlot<'_> {
    /// Declares that this operation has no commit-hook buffer.
    ///
    /// Savepoints taken through it still work at the database level; they simply
    /// report [`supports_hooks`](AtomicOperation::supports_hooks) as `false` and
    /// refuse [`add_commit_hook`](AtomicOperation::add_commit_hook), so callers
    /// take their [`force_execute_pre_commit`] fallback — the same path they
    /// already take on the operation itself.
    ///
    /// [`force_execute_pre_commit`]: hooks::CommitHook::force_execute_pre_commit
    pub fn unsupported() -> Self {
        Self(None)
    }
}

/// An [`AtomicOperation`] scoped to a database `SAVEPOINT` inside a parent [`DbOp`]
/// or another `SavepointOp`.
///
/// Created by [`SavepointOperation::with_savepoint`] /
/// [`SavepointOperation::begin_savepoint`] on any operation — including on another
/// `SavepointOp`, which is how savepoints nest.
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
/// nested or not. They run once, at the root [`commit`](super::DbOp::commit), over
/// the final merged hook set — so `post_commit` still only fires after a durable
/// `COMMIT`, and [`on_rollback`] still only fires when the whole transaction is
/// gone.
///
/// [`DbOp`]: super::DbOp
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
    /// Opens the savepoint on a raw connection.
    ///
    /// Taking the bare `&mut db::Connection` — rather than a `&mut Transaction`
    /// — is what lets one implementation serve *every* operation. sqlx tracks
    /// transaction depth on the connection itself, so this correctly issues
    /// `SAVEPOINT` rather than `BEGIN` whenever we are already inside a
    /// transaction, regardless of whether the caller happens to hold a
    /// `Transaction` wrapper (`DbOp`, another `SavepointOp`) or just the
    /// connection (a [`HookOperation`](super::hooks::HookOperation)).
    pub(super) async fn begin(
        conn: &'t mut db::Connection,
        clock: ClockHandle,
        now: Option<chrono::DateTime<chrono::Utc>>,
        parent_hooks: HookParent<'t>,
    ) -> Result<Self, sqlx::Error> {
        Ok(Self {
            tx: conn.begin().await?,
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
        absorb_staged(&mut parent_hooks, staged)
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
    /// has nowhere to fold hooks — a savepoint over a bare [`sqlx::Transaction`]
    /// or any op that returned [`HookSlot::unsupported`], or one nested under a
    /// `HookOperation` on the `force_execute_pre_commit` path.
    fn add_commit_hook<H: hooks::CommitHook>(&mut self, hook: H) -> Result<(), H> {
        if self.parent_hooks.is_none() {
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
            .or_else(|| self.parent_hooks.as_ref()?.get_last::<H>())
    }

    fn supports_hooks(&self) -> bool {
        self.parent_hooks.is_some()
    }

    /// Nesting: an inner savepoint folds into *this* savepoint's staged buffer,
    /// not straight into the root, so an N-deep chain rolls up one level at a
    /// time. `tx`, `staged` and `parent_hooks` are disjoint fields, so the reads
    /// and borrows below coexist — the split that the trait boundary could not
    /// otherwise express.
    ///
    /// Hook capability is **propagated, not assumed**: this savepoint offers its
    /// `staged` buffer to an inner savepoint only if its own chain can ultimately
    /// receive hooks. Handing the buffer over unconditionally would let an inner
    /// savepoint accept a hook that had nowhere to go, so `add_commit_hook` would
    /// report success for work whose `pre_commit`/`post_commit` could never run.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
        let supported = self.parent_hooks.is_some();
        (
            self.tx.connection(),
            HookSlot(supported.then_some(&mut self.staged)),
        )
    }
}

/// Savepoints for every [`AtomicOperation`], derived rather than hand-written.
///
/// This trait has a blanket implementation and no methods to implement — an
/// operation earns savepoints purely by implementing
/// [`AtomicOperation::savepoint_parts`]. `DbOp`, `DbOpWithTime`, `SavepointOp`
/// (nesting), [`HookOperation`], a bare [`sqlx::Transaction`], any
/// [`OpWithTime`] wrapper, and any operation type defined outside this crate all
/// get the same pair with no per-type work.
///
/// ```rust,ignore
/// use es_entity::{AtomicOperation, SavepointOperation};
///
/// // Generic over the operation: callers can pass a DbOp, a SavepointOp
/// // (nesting one level deeper), or a HookOperation from inside a pre_commit.
/// async fn process_all(
///     op: &mut impl AtomicOperation,
///     items: &[Item],
/// ) -> Result<(), sqlx::Error> {
///     for item in items {
///         let _ = op.with_savepoint(async |sp| process_one(sp, item).await).await?;
///     }
///     Ok(())
/// }
/// ```
///
/// [`HookOperation`]: super::hooks::HookOperation
/// [`OpWithTime`]: super::OpWithTime
pub trait SavepointOperation: AtomicOperation {
    /// Runs `f` inside a `SAVEPOINT`, keeping its work on `Ok` and undoing it on
    /// `Err` — see [`DbOp::with_savepoint`](super::DbOp::with_savepoint) for the
    /// full contract, including the two layers of `Result`.
    ///
    /// The returned future is deliberately **not** declared `Send`. Doing so
    /// would require proving `f`'s future `Send` for every `SavepointOp<'_>`
    /// lifetime, which needs the unstable `async_fn_traits` and still fails to
    /// unify under a higher-ranked bound. Auto-trait inference at each call site
    /// handles it instead, so this composes normally inside `Send` futures —
    /// with the same caveat the inherent form already had: a closure capturing
    /// `&self` may need to capture an owned clone instead.
    fn with_savepoint<T, E, F>(
        &mut self,
        f: F,
    ) -> impl Future<Output = Result<Result<T, E>, sqlx::Error>>
    where
        F: AsyncFnOnce(&mut SavepointOp<'_>) -> Result<T, E>,
    {
        async move {
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
    }

    /// Begins a `SAVEPOINT` scope explicitly — see
    /// [`DbOp::begin_savepoint`](super::DbOp::begin_savepoint). Must be finished
    /// with [`release`](SavepointOp::release) or
    /// [`rollback`](SavepointOp::rollback); dropping it rolls back.
    fn begin_savepoint(
        &mut self,
    ) -> impl Future<Output = Result<SavepointOp<'_>, sqlx::Error>> + Send {
        async move {
            // Both reads must complete before the `&mut` borrow below: cloning
            // the handle is what releases the `&self` borrow `clock()` takes.
            let clock = self.clock().clone();
            let now = self.maybe_now();
            let (conn, slot) = self.savepoint_parts();
            SavepointOp::begin(conn, clock, now, slot.0).await
        }
    }
}

impl<T: AtomicOperation + ?Sized> SavepointOperation for T {}
