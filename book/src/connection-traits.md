# Connection Types and Traits

The type of the `<connection>` argument for generated functions is generic, requiring either the `AtomicOperation` or `IntoOneTimeExecutor` trait to be implemented on the type.
There is a blanket implementation that makes every `AtomicOperation` implement `IntoOneTimeExecutor` - but the reverse is _not_ the case.

## AtomicOperation

The `AtomicOperation` trait represents a transactional operation that can execute multiple database operations atomically with consistent snapshots of the data.

```rust,ignore
pub trait AtomicOperation: Send {
    /// The raw connection. The only required method.
    fn connection(&mut self) -> &mut db::Connection;

    /// The connection *and* the commit-hook buffer a nested SAVEPOINT folds
    /// into on release. The default reports "no hook buffer", which is right
    /// for an op that has none and wrong for a wrapper around one that does —
    /// see "Implementing the trait" below.
    fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) { .. }

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
- anything using the [`delegate_atomic_operation!`](#wrapping-another-operation) macro

Every `AtomicOperation` also gets `with_savepoint` / `begin_savepoint` from the blanket `SavepointOperation` trait — see [Savepoints](./savepoints.md).

## Implementing the trait

Most code never implements `AtomicOperation`; it takes `&mut impl AtomicOperation` and is handed a `DbOp`. You implement it when you define your own operation type.

### Wrapping another operation

A type that wraps an operation — a newtype over `&mut DbOp` that seals off `commit()`, a restricted view handed to a callback — should use the `delegate_atomic_operation!` macro rather than writing the impl out:

```rust,ignore
struct FlushOp<'a>(&'a mut es_entity::DbOp<'static>);

es_entity::delegate_atomic_operation!(FlushOp<'_>, { s => s.0 });
```

That is the entire implementation. The macro generates all of `AtomicOperation` — time, clock, executor, commit hooks, `supports_hooks`, savepoints — each reporting the wrapped operation's real capability rather than a trait default. Methods added to `AtomicOperation` later cost you nothing.

Crucially it adds **no accessor of its own**. The wrapped operation stays exactly as private as you made it, so a type that withholds `&mut` access on purpose — to stop callers committing it or swapping it out — keeps that guarantee.

Delegating by hand instead is not just verbose, it is a trap: a method you forget silently inherits its default, so `maybe_now` starts returning `None`, `supports_hooks` returns `false`, or `savepoint_parts` reports no hook buffer while the wrapped op has one. The behaviour changes and nothing fails to compile.

### Enums

An enum choosing between several operations works the same way. Arms may hold different types, because the macro generates the `match` inside each method rather than unifying the arms into one value:

```rust,ignore
es_entity::delegate_atomic_operation!(UseCaseOp<'_, '_>, {
    Self::Owned(op) => op,
    Self::Db(op) => op,
    Self::Savepoint(op) => op,
});
```

Delegating `savepoint_parts` this way is what lets such an enum open a savepoint whatever it currently holds — including nesting when it is already savepoint-backed.

### Generic types

Pass the impl generics in brackets first:

```rust,ignore
es_entity::delegate_atomic_operation!([<'a, T: AtomicOperation>] MyOp<'a, T>, { s => s.inner });
```

### When not to use it

Only for pure delegation — the macro forwards every method. A wrapper that changes behaviour must hand-write the impl; `OpWithTime` and `DbOpWithTime` do, since both override `maybe_now` to report their cached time.

### Owning a connection directly

If your type owns the connection rather than wrapping an operation, implement `AtomicOperation` directly. Only `connection` is required.

If you have no commit-hook buffer, you are already done — the defaulted `savepoint_parts` reports exactly that, savepoints still work at the database level, and `add_commit_hook` inside them refuses so callers take their `force_execute_pre_commit` fallback.

If you *do* keep a hook buffer, override `savepoint_parts`. It returns the connection **and** the buffer together, because a savepoint holds a `&mut` to both for its whole lifetime and two separate `&mut self` accessors could never be live at once — returning the pair lets you split the borrow across your own disjoint fields:

```rust,ignore
fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
    // `conn` and `hooks` are different fields, so both borrows are legal here.
    (&mut self.conn, HookSlot::from(self.hooks.as_mut()))
}
```

### A mismatch is caught, not ignored

Because the default reports "no hook buffer", a type that forwards `supports_hooks` to an operation it wraps but forgets to override `savepoint_parts` would refuse hooks inside every savepoint taken through it, while the wrapped operation supports them fine — a behaviour change with no compile error.

`begin_savepoint` therefore fails with a protocol error when an operation reports `supports_hooks()` but hands back an unsupported slot. The two can only disagree that one way, so the check has no false positives: an op that honestly has no hooks reports `false` on both sides. Using the `delegate_atomic_operation!` macro avoids the situation entirely.

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
