# Connection Types and Traits

The type of the `<connection>` argument for generated functions is generic, requiring either the `AtomicOperation` or `IntoOneTimeExecutor` trait to be implemented on the type.
There is a blanket implementation that makes every `AtomicOperation` implement `IntoOneTimeExecutor` - but the reverse is _not_ the case.

## AtomicOperation

The `AtomicOperation` trait represents a transactional operation that can execute multiple database operations atomically with consistent snapshots of the data.

```rust,ignore
pub trait AtomicOperation: Send {
    /// Function for querying when the operation is taking place - if it is cached.
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        None
    }

    /// Returns the underlying sqlx::Executor implementation.
    fn as_executor(&mut self) -> &mut sqlx::PgConnection;

    /// Registers a commit hook that will run pre_commit before and post_commit after
    /// the transaction commits. Returns Ok(()) if the hook was registered,
    /// Err(hook) if hooks are not supported.
    fn add_commit_hook<H: CommitHook>(&mut self, hook: H) -> Result<(), H> {
        Err(hook)
    }
}
```

Implementations of `AtomicOperation`:
- `&mut sqlx::Transaction<'_, Postgres>`
- `&mut DbOp<'_>`
- `&mut DbOpWithTime<'_>`
- `&mut OpWithTime<'_, Op>` (where `Op: AtomicOperation`)
- `HookOperation<'_>` (used internally by hooks)

## IntoOneTimeExecutor

The `IntoOneTimeExecutor` trait ensures in a typesafe way that only 1 database operation can occur by consuming the inner reference.

Implementations of `IntoOneTimeExecutor`:
- `&PgPool` - checks out a new connection for each operation
- Any type implementing `AtomicOperation` - guarantees consistency across multiple operations

```rust,ignore
async fn find_by_id_in_op<'a, OP>(op: OP, id: EntityId)
where
    OP: IntoOneTimeExecutor<'a>;

async fn create_in_op<OP>(op: &mut OP, new_entity: NewEntity)
where
    OP: AtomicOperation;
```

Both traits wrap access to an `sqlx::Executor` implementation that ultimately executes the query.

## Method Variants

All CRUD `fn`s that `es-entity` generates come in 2 variants:
```rust,ignore
async fn create(new_entity: NewEntity)
async fn create_in_op(<connection>, new_entity: NewEntity)

async fn update(entity: &mut Entity)
async fn update_in_op(<connection>, entity: &mut Entity)

async fn find_by_id(id: EntityId)
async fn find_by_id_in_op(<connection>, id: EntityId)

etc
```

In all cases the `_in_op` variant accepts a first argument that represents the connection to the database.
The non-`_in_op` variant simply wraps the `_in_op` call by passing an appropriate connection argument internally.

## Operation Requirements

In `es-entity` mutating `fn`s generally require 2 roundtrips to update the `index` table and append to the `events` table.
Hence `create_in_op`, `update_in_op` and `delete_in_op` all require `&mut impl AtomicOperation` first arguments.

Most queries on the other hand are executed with 1 round trip (to fetch the events) and thus accept `impl IntoOneTimeExecutor<'_>` first arguments.

Exceptions to this are for `nested` entities which will be explained in a later section.
