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
    PostPersistHookError(/* only if post_persist_hook configured */),
    PostHydrateError(/* only if post_hydrate_hook configured */),
}
```

> `PostPersistHookError` and `PostHydrateError` are only present when the corresponding hook is configured. `PostPersistHookError` wraps `sqlx::Error` by default, or a custom error type if configured via `post_persist_hook(error = "...")`. See [Hooks](./repo-hooks.md) for details.

### Handling constraint violations

When a `create` or `create_all` operation violates a unique constraint, the error is returned as `ConstraintViolation` rather than a raw `Sqlx` error. The `column` field identifies which column caused the violation and the `value` field contains the conflicting value extracted from the PostgreSQL error detail.

```rust,ignore
let result = users.create(new_user).await;
match result {
    Ok(user) => { /* success */ }
    // Column-agnostic check
    Err(e) if e.was_duplicate() => {
        println!("some unique constraint violated");
    }
    Err(e) => return Err(e.into()),
}

// Or check a specific column:
match result {
    Ok(user) => { /* success */ }
    Err(e) if e.was_duplicate_by(UserColumn::Email) => {
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

`UserModifyError` has the same structure as `UserCreateError` (minus `HydrationError` and `PostHydrateError`) and is returned by `update`, `update_all`, and `delete`. `PostPersistHookError` is only present when `post_persist_hook` is configured. It provides the same `was_duplicate`, `was_duplicate_by`, `duplicate_value`, and `was_concurrent_modification` helpers.

### Nested entity errors

For aggregates with nested entities (e.g. `Order` containing `OrderItem`s), `CreateError` and `ModifyError` include additional variants wrapping the child's errors. The `duplicate_value` and `was_concurrent_modification` helpers cascade into nested errors automatically:

```rust,ignore
// If a nested OrderItem creation triggers a constraint violation,
// duplicate_value() still returns the conflicting value:
let val = err.duplicate_value(); // cascades into nested variants
```

The `was_duplicate_by` helper does **not** cascade because nested entities have a different column enum. To check which nested column was violated, match the nested variant directly:

```rust,ignore
match err {
    OrderModifyError::OrderItemsCreate(item_err)
        if item_err.was_duplicate_by(OrderItemColumn::Sku) =>
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
    NotFound { entity: &'static str, column: Option<UserColumn>, value: String },
    HydrationError(EntityHydrationError),
    PostHydrateError(/* only if post_hydrate_hook configured */),
}
```

The `NotFound` variant is returned by `find_by_*` methods when no matching row exists. It includes the entity name, the column searched (as the `UserColumn` enum), and the value that was not found.

### Checking for not-found

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

### Matching on a specific column

Use `was_not_found_by` to check which column was searched, or pattern-match directly on the `NotFound` variant for full control:

```rust,ignore
// Helper method
if e.was_not_found_by(UserColumn::Email) {
    let value = e.not_found_value(); // Option<&str>
    println!("no user with email {}", value.unwrap_or("unknown"));
}

// Pattern matching for custom error conversion
impl From<UserFindError> for AppError {
    fn from(error: UserFindError) -> Self {
        match error {
            UserFindError::NotFound {
                column: Some(UserColumn::Id),
                value,
                ..
            } => Self::UserNotFoundById(value),
            UserFindError::NotFound {
                column: Some(UserColumn::Email),
                value,
                ..
            } => Self::UserNotFoundByEmail(value),
            other => Self::Internal(other.into()),
        }
    }
}
```

Use `maybe_find_by_*` to get `Ok(None)` instead of an error when the entity doesn't exist.

## UserQueryError

```rust,ignore
pub enum UserQueryError {
    Sqlx(sqlx::Error),
    HydrationError(EntityHydrationError),
    CursorDestructureError(CursorDestructureError),
    PostHydrateError(/* only if post_hydrate_hook configured */),
}
```

Returned by paginated list operations (`list_by_*`, `list_for_*`, `list_for_filters`). The `CursorDestructureError` variant occurs when a pagination cursor cannot be decoded.
