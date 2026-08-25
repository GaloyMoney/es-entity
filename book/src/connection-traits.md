# Connection Types and Traits

The type of the `<connection>` argument for generated functions is generic, requiring either the `AtomicOperation` or `IntoOneTimeExecutor` trait to be implemented on the type.
There is a blanket implementation that makes every `AtomicOperation` implement `IntoOneTimeExecutor` - but the reverse is _not_ the case.

## AtomicOperation

The `AtomicOperation` trait represents a transactional operation that can execute multiple database operations atomically with consistent snapshots of the data.

```rust,ignore
pub trait AtomicOperation: Send {
    /// The raw connection. The only required method besides `savepoint_parts`.
    fn connection(&mut self) -> &mut db::Connection;

    /// The connection *and* the commit-hook buffer a nested SAVEPOINT folds
    /// into on release — see "Implementing the trait" below.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>);

    /// When the operation is taking place, if that time is cached.
    fn maybe_now(&self) -> Option<chrono::DateTime<chrono::Utc>> { None }

    /// The clock to read time from. Defaults to the global clock.
    fn clock(&self) -> &ClockHandle { .. }

    /// The executor statements should run through — annotates each statement
    /// with the current span's trace context.
    fn as_executor(&mut self) -> OneTimeExecutor<'_, &mut db::Connection> { .. }

    /// Registers a commit hook that runs pre_commit before and post_commit
    /// after the transaction commits. `Err(hook)` if hooks are unsupported.
    fn add_commit_hook<H: CommitHook>(&mut self, hook: H) -> Result<(), H> {
        Err(hook)
    }

    /// Shared access to the currently-accumulating hook of type `H`.
    fn commit_hook<H: CommitHook>(&self) -> Option<&H> { None }

    /// Whether `add_commit_hook` can actually register a hook.
    fn supports_hooks(&self) -> bool { false }
}
```

Implementations of `AtomicOperation`:
- `sqlx::Transaction<'_, Postgres>`
- `DbOp<'_>`
- `DbOpWithTime<'_>`
- `OpWithTime<'_, Op>` (where `Op: AtomicOperation`)
- `HookOperation<'_>` (used internally by hooks)
- `SavepointOp<'_>` (see [Savepoints](./savepoints.md))
- anything implementing [`WrapsOperation`](#wrapping-another-operation)

Every `AtomicOperation` also gets `with_savepoint` / `begin_savepoint` from the blanket `SavepointOperation` trait — see [Savepoints](./savepoints.md).

## Implementing the trait

Most code never implements `AtomicOperation`; it takes `&mut impl AtomicOperation` and is handed a `DbOp`. You implement it when you define your own operation type.

### Wrapping another operation

A type that wraps an operation — a newtype over `&mut DbOp` that seals off `commit()`, a restricted view handed to a callback — should implement `WrapsOperation` rather than `AtomicOperation`:

```rust,ignore
struct FlushOp<'a>(&'a mut es_entity::DbOp<'static>);

impl<'a> WrapsOperation for FlushOp<'a> {
    type Inner = es_entity::DbOp<'static>;
    fn op(&self) -> &Self::Inner { self.0 }
    fn op_mut(&mut self) -> &mut Self::Inner { self.0 }
}
```

That is the entire implementation. A blanket impl derives all of `AtomicOperation` from those two accessors — time, clock, executor, commit hooks, `supports_hooks`, savepoints — each reporting the wrapped operation's real capability rather than a trait default. Methods added to `AtomicOperation` later cost you nothing.

Delegating by hand instead is not just verbose, it is a trap: a method you forget silently inherits its default, so `maybe_now` starts returning `None` or `supports_hooks` returns `false`. The behaviour changes and nothing fails to compile.

`WrapsOperation` is all-or-nothing. A type cannot implement it *and* override one method, since that needs its own conflicting `impl AtomicOperation`. Two cases therefore hand-write the impl:

- **Wrappers that genuinely differ.** `OpWithTime` and `DbOpWithTime` override `maybe_now` to report their cached time.
- **Enums that dispatch over several operations.** A single associated `Inner` cannot cover several variants, so each method is a `match`:

  ```rust,ignore
  impl AtomicOperation for UseCaseOp<'_, '_> {
      fn connection(&mut self) -> &mut db::Connection {
          match self {
              Self::Owned(op) => op.connection(),
              Self::Db(op) => op.connection(),
              Self::Savepoint(op) => op.connection(),
          }
      }

      fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
          match self {
              Self::Owned(op) => op.savepoint_parts(),
              Self::Db(op) => op.savepoint_parts(),
              Self::Savepoint(op) => op.savepoint_parts(),
          }
      }

      // ...and so on for the remaining methods
  }
  ```

  Dispatching `savepoint_parts` this way is what lets such an enum open a savepoint whatever it currently holds — including nesting when it is already savepoint-backed.

### Owning a connection directly

If your type owns the connection rather than wrapping an operation, implement `AtomicOperation` directly. Only `connection` and `savepoint_parts` are required; the rest have defaults. `savepoint_parts` returns the connection **and** the commit-hook buffer together, because a savepoint holds a `&mut` to both for its whole lifetime and two separate `&mut self` accessors could never be live at once — returning the pair lets you split the borrow across your own disjoint fields:

```rust,ignore
fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
    // `conn` and `hooks` are different fields, so both borrows are legal here.
    (&mut self.conn, HookSlot::from(self.hooks.as_mut()))
}
```

If you have no commit-hook buffer, return `HookSlot::unsupported()`: savepoints still work at the database level, and `add_commit_hook` inside them refuses so callers take their `force_execute_pre_commit` fallback — exactly as they already do on the operation itself.

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
