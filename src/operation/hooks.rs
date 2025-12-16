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
//! # use sqlx::PgPool;
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
//! # async fn example(pool: PgPool) -> Result<(), sqlx::Error> {
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
    collections::HashMap,
    future::Future,
    pin::Pin,
};

use super::AtomicOperation;

/// Type alias for boxed async futures.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Trait for implementing custom commit hooks that execute before and after transaction commits.
///
/// Hooks execute in order: [`pre_commit()`](Self::pre_commit) → database commit → [`post_commit()`](Self::post_commit).
/// Multiple hooks of the same type can be merged via [`merge()`](Self::merge).
///
/// See module-level documentation for a complete example.
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
/// Implements [`AtomicOperation`] to allow executing database queries within the transaction.
pub struct HookOperation<'c> {
    now: Option<chrono::DateTime<chrono::Utc>>,
    conn: &'c mut sqlx::PgConnection,
}

impl<'c> HookOperation<'c> {
    fn new(op: &'c mut impl AtomicOperation) -> Self {
        Self {
            now: op.maybe_now(),
            conn: op.as_executor(),
        }
    }
}

impl<'c> AtomicOperation for HookOperation<'c> {
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.now
    }

    fn as_executor(&mut self) -> &mut sqlx::PgConnection {
        self.conn
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

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool;

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

    fn try_merge(&mut self, other: &mut dyn DynHook) -> bool {
        let other_h = other
            .as_any_mut()
            .downcast_mut::<H>()
            .expect("hook type mismatch");
        self.merge(other_h)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct CommitHooks {
    hooks: HashMap<TypeId, Vec<Box<dyn DynHook>>>,
}

impl CommitHooks {
    pub fn new() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }

    pub(super) fn add<H: CommitHook>(&mut self, hook: H) {
        let type_id = TypeId::of::<H>();
        let hooks_vec = self.hooks.entry(type_id).or_default();

        let mut new_hook: Box<dyn DynHook> = Box::new(hook);

        if let Some(last) = hooks_vec.last_mut()
            && last.try_merge(new_hook.as_mut())
        {
            return;
        }

        hooks_vec.push(new_hook);
    }

    pub(super) async fn execute_pre(
        mut self,
        op: &mut impl AtomicOperation,
    ) -> Result<PostCommitHooks, sqlx::Error> {
        let mut op = HookOperation::new(op);
        let mut post_hooks = Vec::new();

        for (_, hooks_vec) in self.hooks.drain() {
            for hook in hooks_vec {
                let (new_op, hook) = hook.pre_commit_boxed(op).await?;
                op = new_op;
                post_hooks.push(hook);
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

pub struct PostCommitHooks {
    hooks: Vec<Box<dyn DynHook>>,
}

impl PostCommitHooks {
    pub(super) fn execute(self) {
        for hook in self.hooks {
            hook.post_commit_boxed();
        }
    }
}
