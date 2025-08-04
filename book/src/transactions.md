# Transactions

One big advantage of using an ACID compilant database (such as Postgres) is transactional guarantees.

That is you can execute multiple writes atomically or multiple succesive queries can view a consistent snapshot of your data.

The `sqlx` struct that manages this is the [`Transaction`](https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html) that is typically aquired from a pool.

All CRUD `fn`s that`es-entity` generates come in 2 variants

EG:
```rust,ignore
async fn fn create(new_entity: NewEntity)
async fn fn create_in_op(<conection>, new_entity: NewEntity)

async fn fn update(entity: &mut Entity)
async fn fn update_in_op(<conection>, entity: &mut Entity)

async fn fn find_by_id(id: EntityId)
async fn fn find_by_id_in_op(<connection>, id: EntityId)

etc
```

In all cases the `_in_op` variant accepts a first argument that represents the connection to the database.
The non-`_in_op` variant simply wraps the `_in_op` call by passing an appropriate connection argument.

The type of the argument is generic requiring either the `AtomicOperation` or `IntoOneTimeExecutor` trait to be implemented on the type.
There is a blanket implementation that makes every `AtomicOperation` implement `IntoOneTimeExecutor` - but the reverse is _not_ the case.

eg:
```rust,ignore
async fn fn create_in_op<OP>(op: &mut OP, new_entity: NewEntity)
where
    OP: AtomicOperation
```

or
```rust,ignore
async fn fn find_by_id_in_op<'a, OP>(op: OP, id: EntityId)
where
    OP: IntoOneTimeExecutor<'a>
```

Both traits wrap access to an `sqlx::Executor` implementation that ultimately execute the query.

The difference is that the `IntoOneTimeExecutor` trait ensures in a typesafe way that only 1 database operation can occur by consuming the inner reference.

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

// Rediculous example of 2 operations that we want to execute atomically
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

    // It can only execute 1 roundtrip per use as it will
    // check out a new connection from the pool for each operation
    // hence we cannot pass it to `fn`'s that need AtomicOperation
    // let _ = delete_and_count(&pool).await?; // <- won't compile

    // &mut sqlx::PgTransaction implements AtomicOperation
    // so we can use it for both `fns`
    let mut tx = pool.begin().await?;
    let _ = count_users(&mut tx).await?;

    let some_existing_user_id = uuid::Uuid::new_v4();
    let _ = delete_and_count(&mut tx, some_existing_user_id).await?;

    // Types that implement AtomicOperation can be used for IntoOneTimeExecutor
    // multiple times
    let _ = count_users(&mut tx).await?;

    // Don't forget to commit the operation or the change won't become visible
    tx.commit().await?;

    Ok(())
}
```

In `es-entity` mutating `fn`s generally require 2 roundtrips to update the `index` table and append to the `events` table.
Hence `create_in_op`, `update_in_op` and `delete_in_op` all require `&mut impl AtomicOperation` first arguments.

Most queries are executed with 1 round trip (to fetch the events) and thus accept `impl IntoOneTimeExecutor<'_>` first arguments.

Exceptions to this are for `nested` entities which have will be explained in a later section.
