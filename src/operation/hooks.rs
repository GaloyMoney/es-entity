//! Commit hooks for executing custom logic before and after transaction commits.
//!
//! This module provides the [`CommitHook`] trait and supporting types that allow you to
//! register hooks that execute during the commit lifecycle of a transaction. This is useful for:
//!
//! - Publishing events to message queues after successful commits
//! - Updating caches
//! - Triggering side effects that should only occur if the transaction succeeds
//! - Accumulating operations across multiple entity updates in a transaction
//!
//! # Hook Lifecycle
//!
//! 1. **Registration**: Hooks are registered using [`AtomicOperation::add_commit_hook()`]
//! 2. **Merging**: Multiple hooks of the same type may be merged via [`CommitHook::merge()`]
//! 3. **Pre-commit**: [`CommitHook::pre_commit()`] executes before the transaction commits
//! 4. **Commit**: The underlying database transaction is committed
//! 5. **Post-commit**: [`CommitHook::post_commit()`] executes after successful commit
//!
//! If the commit **fails** instead — either a later hook's `pre_commit` errors (the
//! transaction is rolled back first) or the `COMMIT` itself errors — then
//! [`CommitHook::on_rollback()`] is fired on every hook whose `pre_commit` had already
//! completed, in registration order, in place of `post_commit`. It is a synchronous,
//! infallible, signal-only callback (see its docs).
//!
//! # Hook Ordering
//!
//! Hooks execute in **registration order** — per hook, not per type:
//!
//! 1. Hooks run in the order they were added to the operation, regardless of their
//!    type. A hook that refuses to merge ([`CommitHook::merge()`] returns `false`)
//!    executes at its own (later) registration position, even when an earlier hook
//!    of the same type exists.
//! 2. A hook that merges executes at the position of the hook it merged into (the
//!    earlier one). For always-merging hook types this means the type's position is
//!    anchored by its **first** registration in the operation; all later
//!    registrations fold into that position.
//! 3. [`CommitHook::post_commit()`] hooks run in the same order as their
//!    [`CommitHook::pre_commit()`] counterparts (registration order).
//! 4. A hook's `pre_commit` may itself register further hooks — see
//!    [`AtomicOperation::add_commit_hook()`] on the [`HookOperation`] it is handed.
//!    Those join the **tail of the same commit pass** rather than freezing the set
//!    before the first `pre_commit` runs. See "Re-entrant Registration" below.
//!
//! Registration order is a determinism guarantee, not a priority mechanism — there
//! is no way to reorder hooks independently of the order in which they were added.
//!
//! # Re-entrant Registration
//!
//! [`AtomicOperation::add_commit_hook()`] succeeds on the [`HookOperation`] passed to
//! a `pre_commit` that is running as part of a real commit pass — a hook can register
//! more hooks, and they join the pass instead of being silently dropped or forced to
//! run outside the commit lifecycle:
//!
//! - A newly-registered hook merges into a **still-pending** hook of the same type,
//!   keeping that hook's queue position — indistinguishable from having registered it
//!   there directly. An **already-executed** hook of that type is never a merge
//!   target (its `pre_commit` already ran), so registering that type again after its
//!   own execution always starts a fresh instance, which runs its own `pre_commit`
//!   later in the same pass.
//! - A bound ([`MAX_HOOK_GENERATIONS`] re-entrant generations) guards against a
//!   registration cycle (A registers B, B registers A, …), which would otherwise grow
//!   the queue forever inside an open transaction. Exceeding it fails the commit
//!   loudly instead of hanging.
//! - This only applies to a real commit pass. [`CommitHook::force_execute_pre_commit()`]
//!   — the escape hatch for ops that don't support hooks at all — still returns
//!   `Err` from `add_commit_hook`, because there is no pass for a registered hook to
//!   join; its `post_commit`/`on_rollback` would simply never run.
//!
//! # Savepoints
//!
//! Hooks registered on a [`SavepointOp`] are staged and only enter the parent
//! operation's set — through the same registration/merge path — when the savepoint
//! is released; a rolled-back savepoint discards them. No callback runs at a
//! savepoint boundary, so the lifecycle above is unchanged: one `pre_commit` pass at
//! the parent's commit, `post_commit` only after a durable `COMMIT`. See
//! [`SavepointOp`] for details.
//!
//! [`SavepointOp`]: super::SavepointOp
//!
//! # Examples
//!
//! ## Hook with Database Operations and Channel-Based Publishing
//!
//! This example shows a complete event publishing hook that:
//! - Stores events in the database during pre-commit (within the transaction)
//! - Sends events to a channel during post-commit for async processing
//! - Merges multiple hook instances to batch operations
//!
//! Note: `post_commit()` is synchronous and cannot fail, so it's best used for
//! fire-and-forget operations like sending to channels. A background task can then
//! handle the async work of publishing to external systems.
//!
//! ```
//! use es_entity::{AtomicOperation, operation::hooks::{CommitHook, HookOperation, PreCommitRet}};
//!
//! #[derive(Debug, Clone)]
//! struct Event {
//!     entity_id: uuid::Uuid,
//!     event_type: String,
//! }
//!
//! #[derive(Debug)]
//! struct EventPublisher {
//!     events: Vec<Event>,
//!     // Channel sender for publishing events to a background processor
//!     // In production, this might be tokio::sync::mpsc::Sender or similar
//!     tx: std::sync::mpsc::Sender<Event>,
//! }
//!
//! impl CommitHook for EventPublisher {
//!     async fn pre_commit(self, mut op: HookOperation<'_>)
//!         -> Result<PreCommitRet<'_, Self>, sqlx::Error>
//!     {
//!         // Store events in the database within the transaction
//!         // If the transaction fails, these inserts will be rolled back
//!         for event in &self.events {
//!             sqlx::query!(
//!                 "INSERT INTO hook_events (entity_id, event_type, created_at) VALUES ($1, $2, NOW())",
//!                 event.entity_id,
//!                 event.event_type
//!             )
//!             .execute(op.as_executor())
//!             .await?;
//!         }
//!
//!         PreCommitRet::ok(self, op)
//!     }
//!
//!     fn post_commit(self) {
//!         // Send events to a channel for async processing
//!         // This only runs if the transaction succeeded
//!         // Channel sends are fast and don't block; a background task handles publishing
//!         for event in self.events {
//!             // In production, handle send failures appropriately (logging, metrics, etc.)
//!             // The channel might be bounded to apply backpressure
//!             let _ = self.tx.send(event);
//!         }
//!     }
//!
//!     fn merge(&mut self, other: &mut Self) -> bool {
//!         // Merge multiple EventPublisher hooks into one to batch operations
//!         self.events.append(&mut other.events);
//!         true
//!     }
//! }
//!
//! // Separate background task for async event publishing
//! // async fn event_publisher_task(mut rx: tokio::sync::mpsc::Receiver<Event>) {
//! //     while let Some(event) = rx.recv().await {
//! //         // Publish to Kafka, RabbitMQ, webhooks, etc.
//! //         // Handle failures with retries, dead-letter queues, etc.
//! //         match publish_to_external_system(&event).await {
//! //             Ok(_) => log::info!("Published event: {:?}", event),
//! //             Err(e) => log::error!("Failed to publish event: {:?}", e),
//! //         }
//! //     }
//! // }
//! ```
//!
//! ## Usage
//!
//! ```no_run
//! # use es_entity::{AtomicOperation, DbOp, operation::hooks::{CommitHook, HookOperation, PreCommitRet}};
//! # use es_entity::db;
//! # #[derive(Debug, Clone)]
//! # struct Event { entity_id: uuid::Uuid, event_type: String }
//! # #[derive(Debug)]
//! # struct EventPublisher { events: Vec<Event>, tx: std::sync::mpsc::Sender<Event> }
//! # impl CommitHook for EventPublisher {
//! #     async fn pre_commit(self, mut op: HookOperation<'_>) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
//! #         for event in &self.events {
//! #             sqlx::query!(
//! #                 "INSERT INTO hook_events (entity_id, event_type, created_at) VALUES ($1, $2, NOW())",
//! #                 event.entity_id, event.event_type
//! #             ).execute(op.as_executor()).await?;
//! #         }
//! #         PreCommitRet::ok(self, op)
//! #     }
//! #     fn post_commit(self) { for event in self.events { let _ = self.tx.send(event); } }
//! #     fn merge(&mut self, other: &mut Self) -> bool { self.events.append(&mut other.events); true }
//! # }
//! # async fn example(pool: db::Pool) -> Result<(), sqlx::Error> {
//! let user_id = uuid::Uuid::nil();
//! let (tx, _rx) = std::sync::mpsc::channel();
//! let mut op = DbOp::init(&pool).await?;
//!
//! // Add first hook
//! op.add_commit_hook(EventPublisher {
//!     events: vec![Event { entity_id: user_id, event_type: "user.created".to_string() }],
//!     tx: tx.clone(),
//! }).expect("could not add hook");
//!
//! // Add second hook - will merge with the first
//! op.add_commit_hook(EventPublisher {
//!     events: vec![Event { entity_id: user_id, event_type: "email.sent".to_string() }],
//!     tx: tx.clone(),
//! }).expect("could not add hook");
//!
//! // Both hooks merge into one, events are stored in DB, then sent to channel
//! op.commit().await?;
//! # Ok(())
//! # }
//! ```

use std::{
    any::{Any, TypeId},
    collections::VecDeque,
    future::Future,
    pin::Pin,
};

use crate::db;

use super::AtomicOperation;

/// Type alias for boxed async futures.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait for implementing custom commit hooks that execute before and after transaction commits.
///
/// Hooks execute in order: [`pre_commit()`](Self::pre_commit) → database commit → [`post_commit()`](Self::post_commit).
/// Multiple hooks of the same type can be merged via [`merge()`](Self::merge).
///
/// Hooks registered on the same operation execute in registration order — see the
/// [module-level documentation](self#hook-ordering) for the full ordering contract
/// and a complete example.
pub trait CommitHook: Send + 'static + Sized {
    /// Called before the transaction commits. Can perform database operations.
    ///
    /// Errors returned here will roll back the transaction.
    fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> impl Future<Output = Result<PreCommitRet<'_, Self>, sqlx::Error>> + Send {
        async { PreCommitRet::ok(self, op) }
    }

    /// Called after successful commit. Cannot fail, not async.
    fn post_commit(self) {
        // Default: do nothing
    }

    /// Called when the operation's commit has **failed** after this hook's
    /// [`pre_commit()`](Self::pre_commit) had already completed successfully.
    ///
    /// Two situations trigger it:
    /// 1. A *later* hook's `pre_commit` returned an error. The transaction has
    ///    been **rolled back before this runs** — so any downstream side effect
    ///    the signal triggers never contends with the failed transaction's own
    ///    locks.
    /// 2. The `COMMIT` itself returned an error. The transaction is over
    ///    server-side either way (it may have landed despite the client error,
    ///    or aborted), so downstream side effects must be idempotent against a
    ///    possibly-landed commit.
    ///
    /// Signal-only, synchronous and infallible — mirrors
    /// [`post_commit()`](Self::post_commit). Do **not** perform database work
    /// here (the transaction is gone and there is no async context); hand work
    /// to an out-of-band task via a channel send / flag set instead.
    ///
    /// Not called when `pre_commit` never ran (an operation dropped without
    /// `commit()` produced no effects to compensate), nor for the hook whose
    /// own `pre_commit` failed — that hook is consumed by the failing call and
    /// must signal from its own error branch.
    fn on_rollback(self) {
        // Default: do nothing
    }

    /// Try to merge another hook of the same type into this one.
    ///
    /// Returns `true` if merged (other will be dropped), `false` if not (both execute separately).
    fn merge(&mut self, _other: &mut Self) -> bool {
        false
    }

    /// Execute the hook immediately, bypassing the hook system.
    ///
    /// Useful when [`AtomicOperation::add_commit_hook()`] returns `Err(hook)`.
    fn force_execute_pre_commit(
        self,
        op: &mut impl AtomicOperation,
    ) -> impl Future<Output = Result<Self, sqlx::Error>> + Send {
        async {
            let hook_op = HookOperation::new(op);
            Ok(self.pre_commit(hook_op).await?.hook)
        }
    }
}

/// Wrapper around a database connection passed to [`CommitHook::pre_commit()`].
///
/// Implements [`AtomicOperation`] to allow executing database queries within the
/// transaction. Whether it also supports registering *further* commit hooks depends
/// on how it was constructed — see the `staged` field below and "Re-entrant
/// Registration" in the [module docs](self).
pub struct HookOperation<'c> {
    now: Option<chrono::DateTime<chrono::Utc>>,
    conn: &'c mut db::Connection,
    /// Hooks registered by the `pre_commit` currently holding this op, staged for
    /// the enclosing commit pass to fold into its own not-yet-executed tail once
    /// that `pre_commit` returns.
    ///
    /// `Some` when this op was constructed by [`CommitHooks::execute_pre`] (a real
    /// commit pass is running and can fold staged hooks in). `None` on the
    /// [`force_execute_pre_commit`] path — there is no pass to fold into, so
    /// registration there must keep failing, so callers take their
    /// immediate-execution fallback instead of silently losing a hook's
    /// `post_commit`/`on_rollback`.
    ///
    /// [`force_execute_pre_commit`]: CommitHook::force_execute_pre_commit
    staged: Option<CommitHooks>,
}

impl<'c> HookOperation<'c> {
    /// Force-execute path: no commit pass to fold into, so registering further
    /// hooks stays unsupported.
    fn new(op: &'c mut impl AtomicOperation) -> Self {
        Self {
            now: op.maybe_now(),
            conn: op.connection(),
            staged: None,
        }
    }

    /// Commit-pass path: hooks registered while this op is threaded through
    /// [`CommitHooks::execute_pre`] accumulate here.
    fn staging(op: &'c mut impl AtomicOperation) -> Self {
        Self {
            now: op.maybe_now(),
            conn: op.connection(),
            staged: Some(CommitHooks::new()),
        }
    }

    /// Takes whatever the `pre_commit` that just returned staged, leaving a fresh
    /// empty buffer behind for the next hook. `None` on the force-execute path.
    fn drain_staged(&mut self) -> Option<CommitHooks> {
        Some(std::mem::take(self.staged.as_mut()?))
    }
}

impl<'c> AtomicOperation for HookOperation<'c> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn connection(&mut self) -> &mut db::Connection {
        self.conn
    }

    fn add_commit_hook<H: CommitHook>(&mut self, hook: H) -> Result<(), H> {
        match self.staged.as_mut() {
            Some(staged) => {
                staged.add(hook);
                Ok(())
            }
            None => Err(hook),
        }
    }

    fn commit_hook<H: CommitHook>(&self) -> Option<&H> {
        self.staged.as_ref()?.get_last::<H>()
    }

    fn supports_hooks(&self) -> bool {
        self.staged.is_some()
    }
}

/// Return type for [`CommitHook::pre_commit()`].
///
/// Use [`PreCommitRet::ok()`] to construct: `PreCommitRet::ok(self, op)`.
pub struct PreCommitRet<'c, H> {
    op: HookOperation<'c>,
    hook: H,
}

impl<'c, H> PreCommitRet<'c, H> {
    /// Creates a successful pre-commit result.
    pub fn ok(hook: H, op: HookOperation<'c>) -> Result<Self, sqlx::Error> {
        Ok(Self { op, hook })
    }
}

// --- Object-safe internal trait ---
trait DynHook: Send {
    #[allow(clippy::type_complexity)]
    fn pre_commit_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<(HookOperation<'c>, Box<dyn DynHook>), sqlx::Error>>;

    fn post_commit_boxed(self: Box<Self>);

    fn on_rollback_boxed(self: Box<Self>);

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool;

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<H: CommitHook> DynHook for H {
    fn pre_commit_boxed<'c>(
        self: Box<Self>,
        op: HookOperation<'c>,
    ) -> BoxFuture<'c, Result<(HookOperation<'c>, Box<dyn DynHook>), sqlx::Error>> {
        Box::pin(async move {
            let ret = self.pre_commit(op).await?;
            Ok((ret.op, Box::new(ret.hook) as Box<dyn DynHook>))
        })
    }

    fn post_commit_boxed(self: Box<Self>) {
        (*self).post_commit()
    }

    fn on_rollback_boxed(self: Box<Self>) {
        (*self).on_rollback()
    }

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool {
        let other_h = other
            .as_any_mut()
            .downcast_mut::<H>()
            .expect("hook type mismatch");
        self.merge(other_h)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Hooks are stored in a single flat insertion-ordered vec so that
/// [`execute_pre`](Self::execute_pre) runs them in registration order — per hook,
/// not per type. See the [module-level documentation](self#hook-ordering) for the
/// ordering contract.
pub(crate) struct CommitHooks {
    hooks: Vec<(TypeId, Box<dyn DynHook>)>,
}

impl CommitHooks {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub(super) fn add<H: CommitHook>(&mut self, hook: H) {
        self.push_or_merge(TypeId::of::<H>(), Box::new(hook));
    }

    /// Folds the hooks staged by a released [`SavepointOp`] into this buffer.
    ///
    /// Replays them through the same path as [`add`](Self::add), in their
    /// staging order, so the result is indistinguishable from having registered
    /// them on this operation directly: mergeable types accumulate into the
    /// earlier instance (keeping its position), non-mergeable ones append.
    ///
    /// [`SavepointOp`]: super::SavepointOp
    pub(super) fn absorb_staged(&mut self, staged: Self) {
        for (type_id, hook) in staged.hooks {
            self.push_or_merge(type_id, hook);
        }
    }

    fn push_or_merge(&mut self, type_id: TypeId, mut new_hook: Box<dyn DynHook>) {
        // Merge with the most recently added hook of the same type, keeping the
        // existing hook's original (earlier) position in the execution order.
        if let Some((_, existing)) = self.hooks.iter_mut().rev().find(|(t, _)| *t == type_id)
            && existing.try_merge(new_hook.as_mut())
        {
            return;
        }

        self.hooks.push((type_id, new_hook));
    }

    pub(super) fn get_last<H: CommitHook>(&self) -> Option<&H> {
        self.hooks
            .iter()
            .rev()
            .find(|(t, _)| *t == TypeId::of::<H>())
            .and_then(|(_, hook)| hook.as_any().downcast_ref::<H>())
    }

    /// Runs each hook's `pre_commit` in registration order.
    ///
    /// Re-entrant: a hook may register further hooks via
    /// [`AtomicOperation::add_commit_hook`] on the [`HookOperation`] it is handed.
    /// Those join the **tail of this same pass** — merged into a still-pending hook
    /// of the same type (keeping that hook's position), or appended fresh if no
    /// pending hook of that type remains (an already-executed hook is never a merge
    /// target). [`MAX_HOOK_GENERATIONS`] bounds the resulting chain against a
    /// registration cycle. See the [module docs](self#re-entrant-registration).
    ///
    /// On failure the already-executed hooks travel back **with** the error (as
    /// a [`PostCommitHooks`]) instead of being dropped, so the caller can fire
    /// their [`CommitHook::on_rollback`] after rolling the transaction back.
    /// Hooks after the failing one — pending or not yet staged — never ran their
    /// `pre_commit`, produced no effects, and are simply dropped.
    pub(super) async fn execute_pre(
        self,
        op: &mut impl AtomicOperation,
    ) -> Result<PostCommitHooks, (sqlx::Error, PostCommitHooks)> {
        let mut op = HookOperation::staging(op);
        let mut pending: VecDeque<(TypeId, Box<dyn DynHook>, u8)> = self
            .hooks
            .into_iter()
            .map(|(type_id, hook)| (type_id, hook, 0))
            .collect();
        let mut post_hooks = Vec::with_capacity(pending.len());

        while let Some((_, hook, generation)) = pending.pop_front() {
            match hook.pre_commit_boxed(op).await {
                Ok((mut new_op, hook)) => {
                    post_hooks.push(hook);

                    // Fold whatever that `pre_commit` staged into the tail of this
                    // same pass, through the same registration/merge path
                    // `absorb_staged` uses for released savepoints.
                    if let Some(staged) = new_op.drain_staged() {
                        let next_generation = generation.saturating_add(1);
                        for (type_id, staged_hook) in staged.hooks {
                            if let Err(error) = push_or_merge_pending(
                                &mut pending,
                                type_id,
                                staged_hook,
                                next_generation,
                            ) {
                                return Err((error, PostCommitHooks { hooks: post_hooks }));
                            }
                        }
                    }

                    op = new_op;
                }
                Err(error) => {
                    return Err((error, PostCommitHooks { hooks: post_hooks }));
                }
            }
        }

        Ok(PostCommitHooks { hooks: post_hooks })
    }
}

impl Default for CommitHooks {
    fn default() -> Self {
        Self::new()
    }
}

/// Maximum number of re-entrant "generations" a single commit pass will execute.
/// Generation 0 is the hook set registered on the operation before `commit()`
/// starts; a hook staged by a generation-*N* hook's `pre_commit` is generation
/// *N*+1. Bounds a registration cycle (A registers B, B registers A, …), which
/// would otherwise grow the queue forever inside an open transaction — see the
/// [module docs](self#re-entrant-registration).
pub const MAX_HOOK_GENERATIONS: u8 = 8;

/// Merges `hook` into a still-pending hook of the same type — keeping that hook's
/// queue position and generation — or appends it fresh at `generation`. The
/// deferred-execution counterpart of [`CommitHooks::push_or_merge`]: only hooks that
/// have not yet run their `pre_commit` are eligible merge targets, so a type that
/// already executed this pass always gets a fresh instance rather than retroactively
/// absorbing new work into a hook whose `pre_commit` already returned.
///
/// Errors if the fresh instance would exceed [`MAX_HOOK_GENERATIONS`] — the loud,
/// bounded failure mode for a registration cycle instead of an unbounded queue
/// inside an open transaction.
fn push_or_merge_pending(
    pending: &mut VecDeque<(TypeId, Box<dyn DynHook>, u8)>,
    type_id: TypeId,
    mut hook: Box<dyn DynHook>,
    generation: u8,
) -> Result<(), sqlx::Error> {
    if let Some((_, existing, _)) = pending.iter_mut().rev().find(|(t, _, _)| *t == type_id)
        && existing.try_merge(hook.as_mut())
    {
        return Ok(());
    }

    if generation > MAX_HOOK_GENERATIONS {
        return Err(sqlx::Error::Protocol(format!(
            "commit hook registration exceeded the maximum of {MAX_HOOK_GENERATIONS} \
             re-entrant generations in one commit pass — likely a registration cycle"
        )));
    }

    pending.push_back((type_id, hook, generation));
    Ok(())
}

pub struct PostCommitHooks {
    hooks: Vec<Box<dyn DynHook>>,
}

impl PostCommitHooks {
    pub(super) fn execute(self) {
        for hook in self.hooks {
            hook.post_commit_boxed();
        }
    }

    /// Fires [`CommitHook::on_rollback`] on each already-pre_committed hook in
    /// registration order (same order as [`execute`](Self::execute)). Sync and
    /// infallible, mirroring `execute`.
    pub(super) fn execute_rollback(self) {
        for hook in self.hooks {
            hook.on_rollback_boxed();
        }
    }
}
