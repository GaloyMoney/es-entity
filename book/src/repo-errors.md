# Error Types

`EsRepo` generates four per-entity error types and a column enum. For an entity called `User`, the macro produces:

| Type | Used by |
|------|---------|
| `UserColumn` | Enum of indexed columns (e.g. `Id`, `Name`, `Email`) |
| `UserCreateError` | `create`, `create_all` |
| `UserModifyError` | `update`, `update_all`, `delete` |
| `UserFindError` | `find_by_*`, `maybe_find_by_*` |
| `UserQueryError` | `find_all`, `list_by_*`, `list_for_*`, `list_for_filters` |

## UserCreateError

```rust,ignore
pub enum UserCreateError {
    Sqlx(sqlx::Error),
    ConstraintViolation {
        column: Option<UserColumn>,
        value: Option<String>,
        inner: sqlx::Error,
    },
    ConcurrentModification,
    HydrationError(EntityHydrationError),
    PostPersistHookError(sqlx::Error),
}
```

### Handling constraint violations

When a `create` or `create_all` operation violates a unique constraint, the error is returned as `ConstraintViolation` rather than a raw `Sqlx` error. The `column` field identifies which column caused the violation and the `value` field contains the conflicting value extracted from the PostgreSQL error detail.

```rust,ignore
let result = users.create(new_user).await;
match result {
    Ok(user) => { /* success */ }
    Err(e) if e.was_duplicate(UserColumn::Email) => {
        let value = e.duplicate_value(); // Option<&str>
        println!("email {} already taken", value.unwrap_or("unknown"));
    }
    Err(e) => return Err(e.into()),
}
```

The macro maps PostgreSQL constraint names to columns automatically using the convention `{table}_{column}_key` for unique constraints and `{table}_pkey` for the primary key. If your constraint uses a non-standard name, specify it explicitly:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        email(ty = "String", constraint = "idx_unique_email"),
    )
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```

### Concurrent modification

When optimistic concurrency control detects a conflict (duplicate event sequence), the error is `ConcurrentModification`:

```rust,ignore
if e.was_concurrent_modification() {
    // retry the operation
}
```

## UserModifyError

`UserModifyError` has the same structure as `UserCreateError` (minus `HydrationError`) and is returned by `update`, `update_all`, and `delete`. It provides the same `was_duplicate`, `duplicate_value`, and `was_concurrent_modification` helpers.

### Nested entity errors

For aggregates with nested entities (e.g. `Order` containing `OrderItem`s), `CreateError` and `ModifyError` include additional variants wrapping the child's errors. The `duplicate_value` and `was_concurrent_modification` helpers cascade into nested errors automatically:

```rust,ignore
// If a nested OrderItem creation triggers a constraint violation,
// duplicate_value() still returns the conflicting value:
let val = err.duplicate_value(); // cascades into nested variants
```

The `was_duplicate` helper does **not** cascade because nested entities have a different column enum. To check which nested column was violated, match the nested variant directly:

```rust,ignore
match err {
    OrderModifyError::OrderItemsCreate(item_err)
        if item_err.was_duplicate(OrderItemColumn::Sku) =>
    {
        let val = item_err.duplicate_value();
    }
    _ => return Err(err.into()),
}
```

## UserFindError

```rust,ignore
pub enum UserFindError {
    Sqlx(sqlx::Error),
    NotFound { entity: &'static str, column: &'static str, value: String },
    HydrationError(EntityHydrationError),
}
```

The `NotFound` variant is returned by `find_by_*` methods when no matching row exists. It includes the entity name, column searched, and the value that was not found:

```rust,ignore
let result = users.find_by_id(some_id).await;
match result {
    Ok(user) => { /* found */ }
    Err(e) if e.was_not_found() => {
        println!("user not found");
    }
    Err(e) => return Err(e.into()),
}
```

Use `maybe_find_by_*` to get `Ok(None)` instead of an error when the entity doesn't exist.

## UserQueryError

```rust,ignore
pub enum UserQueryError {
    Sqlx(sqlx::Error),
    HydrationError(EntityHydrationError),
    CursorDestructureError(CursorDestructureError),
}
```

Returned by paginated list operations (`list_by_*`, `list_for_*`, `list_for_filters`). The `CursorDestructureError` variant occurs when a pagination cursor cannot be decoded.
