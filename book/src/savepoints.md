# Savepoints

A `SAVEPOINT` is a mark inside a transaction that you can roll back to without ending the transaction. `DbOp` exposes this as `with_savepoint`, which makes it possible to process a batch of items in **one** transaction while isolating each item's failure.

This matters for two reasons:

- **Throughput.** A batch of N items processed as N transactions pays N WAL flushes. Processed as one transaction with N savepoints it pays one — savepoints themselves are cheap round trips that never fsync.
- **Poisoning.** Once a statement errors, a Postgres transaction refuses all further statements until it is rolled back. Without savepoints, one bad item takes down every other item sharing the transaction.

## Usage

```rust,ignore
let mut op = DbOp::init(&pool).await?;
let mut outcomes = Vec::with_capacity(items.len());

for item in items {
    // The outer `?`: the savepoint machinery itself failed, so the whole
    // operation is suspect — abandon it rather than commit.
    let res = op
        .with_savepoint(async |op| self.process_in_op(op, item).await)
        .await?;

    // The inner Result: this item's own outcome, already rolled back cleanly
    // if it failed. The loop continues either way.
    outcomes.push(match res {
        Ok(()) => Outcome::Complete,
        Err(e) => Outcome::Retry(e),
    });
}

op.commit().await?;
```

The closure receives a `SavepointOp`, which implements `AtomicOperation` — so existing `*_in_op` methods take it unchanged.

## Two layers of `Result`

`with_savepoint` returns `Result<Result<T, E>, sqlx::Error>`:

| Layer | Meaning | What to do |
|---|---|---|
| Outer `Err(sqlx::Error)` | The `SAVEPOINT` / `RELEASE` / `ROLLBACK TO` failed, or the error was never savepoint-recoverable (e.g. the connection died) | Abandon the operation — do not commit |
| Inner `Err(E)` | The item failed; its writes and staged hooks are gone | Record the outcome, continue the loop |

If the closure fails *and* the rollback fails, the rollback error surfaces as the outer `Err` and the item error is dropped — the poisoned-transaction signal is the one the caller must act on.

## Commit hooks are staged

Hooks registered inside a savepoint — including those repositories register internally via [`post_persist_hook`](./repo-hooks.md) — are **staged**, not added to the parent operation:

- On **release**, the staged hooks are folded into the parent's buffer through the ordinary registration/merge path. A mergeable hook type therefore accumulates across the whole batch exactly as if every item had registered on the parent directly.
- On **rollback**, the staged hooks are dropped. A rolled-back item contributes zero hook state to match its zero database state — no event published for a row that no longer exists.

No hook callback runs at a savepoint boundary. [`pre_commit`](./commit-hooks.md) runs once at the parent's `commit()` over the final merged set, `post_commit` still only after a durable `COMMIT`, and `on_rollback` still only when the whole transaction is gone.

While a savepoint is open, `commit_hook::<H>()` reads the staged buffer first and falls back to the parent's — so an item sees the state accumulated by earlier, already-released items. A hook type present in both is reported as the staged instance until release merges them.

## Collecting outcomes

The closure may borrow from its environment, but **host-side mutations do not unwind with the savepoint**. Pushing an outcome inside the closure before a later statement fails would report success for work that was undone. Return the verdict through `Ok`/`Err` and record it outside, as in the example above.

## Explicit form

When the closure form doesn't fit, `begin_savepoint()` returns the `SavepointOp` directly. It must be finished with `release()` (keep the work) or `rollback()` (discard it); dropping it rolls back.

```rust,ignore
let mut sp = op.begin_savepoint().await?;
self.process_in_op(&mut sp, item).await?;
sp.release().await?;
```

## Every operation has savepoints

`with_savepoint` and `begin_savepoint` come from the `SavepointOperation` trait, which is blanket-implemented for **every** `AtomicOperation`. There is nothing to implement to get them:

| Operation | What a savepoint through it folds hooks into |
|---|---|
| `DbOp` / `DbOpWithTime` | the operation's own commit-hook buffer |
| `SavepointOp` | the enclosing savepoint's staged buffer (nesting) |
| `HookOperation` | the running commit pass, or nothing on the `force_execute_pre_commit` path |
| `OpWithTime<'_, Op>` | whatever the wrapped operation folds into |
| `sqlx::Transaction` | nothing — hooks are refused |
| your own operation type | whatever you forward to |

Because the API lives on a trait, one generic helper serves them all:

```rust,ignore
use es_entity::{AtomicOperation, SavepointOperation};

async fn process_all(
    op: &mut impl AtomicOperation,
    items: &[Item],
) -> Result<(), sqlx::Error> {
    for item in items {
        // Call it with a DbOp, a SavepointOp (nesting a level deeper), or a
        // HookOperation from inside a pre_commit — same code either way.
        let _ = op.with_savepoint(async |sp| process_one(sp, item).await).await?;
    }
    Ok(())
}
```

`DbOp` and `DbOpWithTime` also keep inherent `with_savepoint` / `begin_savepoint` methods, so existing call sites work without importing the trait. Reaching for them on any *other* operation needs `use es_entity::SavepointOperation;`.

### Supporting savepoints on your own operation

Implement one method — `AtomicOperation::savepoint_parts` — and the whole pair follows. Wrapper types forward:

```rust,ignore
impl AtomicOperation for MyOp<'_> {
    // ...

    fn savepoint_parts(&mut self) -> (&mut db::Connection, HookSlot<'_>) {
        self.inner.savepoint_parts()
    }
}
```

It returns the connection *and* the hook buffer together because a `SavepointOp` holds a `&mut` to both for its whole lifetime, and two separate `&mut self` accessors could never be live at once. Returning the pair lets you split the borrow across your own disjoint fields.

An operation with no commit-hook buffer returns `HookSlot::unsupported()`: savepoints still work at the database level, and `add_commit_hook` inside them refuses, so callers take their `force_execute_pre_commit` fallback exactly as they already do on the operation itself.

## Nesting

Because `SavepointOp` is itself an `AtomicOperation`, it gets the same pair — so a savepoint can nest inside another, isolating a sub-item's failure within an already-isolated item, without giving up any of the outer batch's atomicity:

```rust,ignore
op.with_savepoint(async |outer| {
    self.process_in_op(outer, item).await?;

    for sub_item in &item.sub_items {
        // A sub-item's failure unwinds only its own writes — `item`'s own
        // work, and the batch, stay intact either way.
        let _ = outer.with_savepoint(async |inner| {
            self.process_sub_item_in_op(inner, sub_item).await
        }).await?;
    }

    Ok::<_, MyError>(())
}).await?;
```

Releasing an inner savepoint folds its staged hooks into its *immediate* parent's staged buffer, not straight into the root `DbOp` — an N-deep chain rolls up one level at a time, so nothing is visible further out until every enclosing savepoint has itself released.

A [`CommitHook`](./commit-hooks.md)'s own `pre_commit` can nest a savepoint too: `HookOperation` — the type `pre_commit` is handed — is an `AtomicOperation` and so gets the same pair, letting a hook isolate its own multi-statement write the same way application code isolates a batch item. This works even on the [`force_execute_pre_commit`](./commit-hooks.md) escape hatch, where there is no commit pass for a registered hook to join: the raw `SAVEPOINT`/`RELEASE`/`ROLLBACK` still works (it only needs the connection), but `add_commit_hook` inside that savepoint keeps refusing, exactly as it already does on the `HookOperation` directly.

## When not to use savepoints

If every item performs the same statement, `create_all` / `update_all` / `find_all` remain cheaper — one statement for the whole batch beats one savepoint pair per item. Reach for savepoints when items need genuinely per-item logic *and* per-item error isolation.
