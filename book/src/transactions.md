# Transactions

One big advantage of using an ACID compliant database (such as Postgres) is transactional guarantees.

That is you can execute multiple writes atomically or multiple successive queries can view a consistent snapshot of your data.

The `sqlx` struct that manages this is the [`Transaction`](https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html) that is typically acquired from a pool.

Es-entity supports custom types that can wrap a connection while augmenting it with additional custom functionality.

By default the generated `async fn begin_op() -> Result<Op, sqlx::Error>` on `EsRepo` structs returns an `es_entity::DbOp` transaction wrapper that has support for [commit hooks](./commit-hooks.md) and caching of transaction time.

In order to be interoperable with bare `sqlx::Transaction`s as well as custom transaction wrappers all generated functions accept one of 2 traits:
- `AtomicOperation` - representing a transactional operation that needs to be committed.
- `IntoOneTimeExecutor<'_>` - representing a connection that can do 1 DB round trip and has no additional consistency guaranteed.

See [Connection Types and Traits](./connection-traits.md) for details on these traits and their implementations.

## Key Concepts

- **[Connection Traits](./connection-traits.md)**: Learn about `AtomicOperation` and `IntoOneTimeExecutor` traits, method variants (`_in_op` functions), and operation requirements.

- **[DbOp](./db-op.md)**: The default transaction wrapper with support for time caching, nested transactions, and `DbOpWithTime` for guaranteed timestamps.

- **[Commit Hooks](./commit-hooks.md)**: Execute custom logic before and after transaction commits, with support for hook merging and database operations during pre-commit.

## Basic Example

```rust
# extern crate anyhow;
# extern crate sqlx;
# extern crate tokio;
# extern crate es_entity;
# extern crate uuid;
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }
async fn count_users(op: impl es_entity::IntoOneTimeExecutor<'_>) -> anyhow::Result<i64> {
    let row = op.into_executor().fetch_optional(sqlx::query!(
        "SELECT COUNT(*) FROM users"
    )).await?;
    Ok(row.and_then(|r| r.count).unwrap_or(0))
}

// Ridiculous example of 2 operations that we want to execute atomically
async fn delete_and_count(op: &mut impl es_entity::AtomicOperation, id: uuid::Uuid) -> anyhow::Result<i64> {
    sqlx::query!(
        "DELETE FROM users WHERE id = $1",
        id
    ).execute(op.as_executor()).await?;

    let row = sqlx::query!(
        "SELECT COUNT(*) FROM users"
    ).fetch_optional(op.as_executor()).await?;
    Ok(row.and_then(|r| r.count).unwrap_or(0))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pool = init_pool().await?;

    // &sqlx::PgPool implements IntoOneTimeExecutor
    let _ = count_users(&pool).await?;

    // It can only execute 1 roundtrip consistently as it will
    // check out a new connection from the pool for each operation.
    // Hence we cannot pass it to `fn`'s that need AtomicOperation
    // as we cannot guarantee that they will be consistent.
    // let _ = delete_and_count(&pool).await?; // <- won't compile

    // &mut sqlx::PgTransaction implements AtomicOperation
    // so we can use it for both `fns`
    let mut tx = pool.begin().await?;
    let _ = count_users(&mut tx).await?;

    let some_existing_user_id = uuid::Uuid::now_v7();
    let _ = delete_and_count(&mut tx, some_existing_user_id).await?;

    // Don't forget to commit the operation or the change won't become visible
    tx.commit().await?;

    Ok(())
}
```
