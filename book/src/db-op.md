# DbOp - Default Transaction Wrapper

`DbOp` is the default transaction wrapper returned by the generated `begin_op()` method on `EsRepo` structs. It wraps a `sqlx::Transaction` while providing:
- Support for commit hooks
- Caching of transaction time
- Integration with the [clock module](./clock.md) for deterministic testing

## Creating DbOp Instances

```rust,ignore
// Initialize from a pool
let mut op = DbOp::init(&pool).await?;

// Convert from a sqlx::Transaction
let tx = pool.begin().await?;
let op: DbOp = tx.into();

// Or use the generated method on your repo
let mut op = MyEntityRepo::begin_op(&pool).await?;
```

## Time Management

`DbOp` supports caching the transaction timestamp, which is useful for:
- Ensuring consistent timestamps across multiple operations in a transaction
- Deterministic testing with manual time (see [clock module](./clock.md))
- Avoiding multiple `NOW()` database queries

When a global manual clock is installed via `Clock::install_manual()`, `DbOp::init()` automatically caches the manual time.

```rust,ignore
// Get cached time if available
let maybe_time: Option<DateTime<Utc>> = op.maybe_now();

// Transition to DbOpWithTime with specific time
let op_with_time = op.with_time(my_timestamp);

// Transition to DbOpWithTime using Clock::now() (manual or system time)
let op_with_time = op.with_system_time();

// Transition to DbOpWithTime with database time
// Uses manual time if installed, otherwise executes SELECT NOW()
let op_with_time = op.with_db_time().await?;
```

## Savepoints

`DbOp` can scope part of its work to a `SAVEPOINT`, so a failure undoes only that part instead of poisoning the whole transaction:

```rust,ignore
// Ok keeps the work (and its staged hooks), Err undoes it
let res: Result<(), MyError> = op
    .with_savepoint(async |op| self.process_in_op(op, item).await)
    .await?;
```

See [Savepoints](./savepoints.md) for the full semantics, including how commit hooks are staged.

## DbOpWithTime

`DbOpWithTime` is equivalent to `DbOp` but guarantees that a timestamp is cached:

```rust,ignore
pub struct DbOpWithTime<'c> {
    // ...
}

impl<'c> DbOpWithTime<'c> {
    /// The cached DateTime
    pub fn now(&self) -> chrono::DateTime<chrono::Utc>;

    /// Begins a nested transaction
    pub async fn begin(&mut self) -> Result<DbOpWithTime<'_>, sqlx::Error>;

    /// Commits the inner transaction
    pub async fn commit(self) -> Result<(), sqlx::Error>;
}
```

It implements both `AtomicOperation` and `AtomicOperationWithTime` traits.
