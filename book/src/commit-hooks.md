# Commit Hooks

Commit hooks allow you to execute custom logic before and after a transaction commits. This is useful for:
- Publishing events to message queues after successful commits
- Updating caches
- Triggering side effects that should only occur if the transaction succeeds
- Accumulating operations across multiple entity updates in a transaction

## CommitHook Trait

The `CommitHook` trait defines the lifecycle hooks:

```rust,ignore
pub trait CommitHook: Send + 'static + Sized {
    /// Called before the transaction commits. Can perform database operations.
    /// Returns Self so it can be used in post_commit.
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        PreCommitRet::ok(self, op)
    }

    /// Called after the transaction has successfully committed.
    /// Cannot fail, not async.
    fn post_commit(self) {
        // Default: do nothing
    }

    /// Try to merge `other` into `self`.
    /// Returns true if merged (other will be dropped).
    /// Returns false if not merged (both will execute separately).
    fn merge(&mut self, _other: &mut Self) -> bool {
        false
    }
}
```

## Hook Execution Lifecycle

1. **Registration**: Hooks are registered using `add_commit_hook()` on any `AtomicOperation`
2. **Merging**: If multiple hooks of the same type are registered and `merge()` returns `true`, they are merged into a single hook
3. **Pre-commit**: All `pre_commit()` methods are called sequentially before the transaction commits
4. **Commit**: The underlying transaction is committed
5. **Post-commit**: All `post_commit()` methods are called sequentially after successful commit

```rust,ignore
let mut op = DbOp::init(&pool).await?;

// Register a hook
op.add_commit_hook(MyHook { data: "example".to_string() })?;

// Hooks execute when commit is called
op.commit().await?;  // pre_commit runs, then tx.commit(), then post_commit
```

## HookOperation

`HookOperation<'_>` is a wrapper passed to `pre_commit()` that allows hooks to execute database operations:

```rust,ignore
impl CommitHook for MyHook {
    async fn pre_commit(
        self,
        mut op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        // Can execute queries
        let result = sqlx::query!("SELECT COUNT(*) FROM events")
            .fetch_one(op.as_executor())
            .await?;

        PreCommitRet::ok(self, op)
    }
}
```

`HookOperation` implements `AtomicOperation` so it can be passed to any function expecting that trait.

## Hook Merging

Hooks of the same type can be merged by implementing the `merge()` method. This is useful for aggregating operations:

```rust,ignore
struct EventPublisher {
    events: Vec<DomainEvent>,
}

impl CommitHook for EventPublisher {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        // Prepare events for publishing
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        // Publish all events to message queue
        publish_events(self.events);
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        // Combine events from multiple entity updates
        self.events.append(&mut other.events);
        true  // Successfully merged
    }
}

// Usage:
let mut op = DbOp::init(&pool).await?;
op.add_commit_hook(EventPublisher { events: vec![event1] })?;
op.add_commit_hook(EventPublisher { events: vec![event2, event3] })?;
// When commit() is called, hooks merge and publish all 3 events together
op.commit().await?;
```

## Fallback for Non-Supporting Operations

Not all `AtomicOperation` implementations support hooks. If `add_commit_hook()` returns `Err(hook)`, you can force immediate execution:

```rust,ignore
let mut tx = pool.begin().await?;  // Plain sqlx transaction doesn't support hooks

match tx.add_commit_hook(my_hook) {
    Ok(()) => {
        // Hook registered, will run on commit
    }
    Err(hook) => {
        // Hooks not supported, execute immediately
        let hook = hook.force_execute_pre_commit(&mut tx).await?;
        tx.commit().await?;
        hook.post_commit();
    }
}
```

## Complete Example

```rust,ignore
use es_entity::operation::{DbOp, hooks::{CommitHook, HookOperation, PreCommitRet}};

#[derive(Debug)]
struct EventPublisher {
    events: Vec<String>,
}

impl CommitHook for EventPublisher {
    async fn pre_commit(
        self,
        op: HookOperation<'_>,
    ) -> Result<PreCommitRet<'_, Self>, sqlx::Error> {
        // Could validate events or store them in a staging table
        PreCommitRet::ok(self, op)
    }

    fn post_commit(self) {
        // Publish events only after successful commit
        for event in self.events {
            println!("Publishing event: {}", event);
            // actual_publish_to_queue(event);
        }
    }

    fn merge(&mut self, other: &mut Self) -> bool {
        // Combine events from multiple operations
        self.events.append(&mut other.events);
        true
    }
}

async fn example_with_hooks(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut op = DbOp::init(pool).await?;

    // Multiple updates might each register hooks
    op.add_commit_hook(EventPublisher {
        events: vec!["user.created".to_string()]
    })?;

    op.add_commit_hook(EventPublisher {
        events: vec!["notification.sent".to_string()]
    })?;

    // When we commit, hooks merge and execute together
    op.commit().await?;
    // Output: Publishing event: user.created
    //         Publishing event: notification.sent

    Ok(())
}
```
