# Repo Hooks

`EsRepo` supports two optional hooks that run during entity lifecycle operations. Both are configured as attributes on the `#[es_repo(...)]` derive macro.

| Hook | Runs | Signature | Use case |
|------|------|-----------|----------|
| `post_persist_hook` | After events are persisted (inside the transaction) | `async fn(&self, &mut OP, &Entity, LastPersisted<Event>) -> Result<(), E>` | Auditing, side-effect recording, cross-entity writes |
| `post_hydrate_hook` | After an entity is reconstructed from events | `fn(&self, &Entity) -> Result<(), E>` | Validation against external config, policy enforcement |

## post_persist_hook

Runs after events have been written to the database but before the entity is returned to the caller. The hook executes inside the same transaction, so it can perform additional database operations or reject the persist.

### Configuration

```rust,ignore
// Simple syntax (error defaults to sqlx::Error):
#[es_repo(entity = "User", post_persist_hook = "on_persist")]

// Explicit syntax with default error:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist"))]

// Explicit syntax with custom error type:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist", error = "MyPersistError"))]
```

### Hook method

The method must be defined on the repo struct with this signature:

```rust,ignore
impl Users {
    async fn on_persist<OP: es_entity::AtomicOperation>(
        &self,
        op: &mut OP,
        entity: &User,
        new_events: es_entity::events::LastPersisted<'_, UserEvent>,
    ) -> Result<(), MyPersistError> {
        // Inspect newly persisted events, write audit records, etc.
        for event in new_events {
            // ...
        }
        Ok(())
    }
}
```

### Error propagation

When the hook returns an error it is wrapped in the `PostPersistHookError` variant of `CreateError` or `ModifyError`:

```rust,ignore
match users.create(new_user).await {
    Err(e) => {
        // e is UserCreateError::PostPersistHookError(MyPersistError)
        println!("persist hook failed: {e}");
    }
    Ok(user) => { /* success */ }
}
```

## post_hydrate_hook

Runs synchronously every time an entity is reconstructed from its event stream — on `create`, `find_by_*`, `list_by_*`, `list_for_*`, and `find_all`. This makes it suitable for invariant checks that depend on external state (e.g. configuration or governance rules) rather than the entity's own events.

### Configuration

```rust,ignore
#[es_repo(
    entity = "User",
    post_hydrate_hook(method = "validate_user", error = "UserValidationError")
)]
```

Both `method` and `error` are required.

### Hook method

The method is synchronous and receives a shared reference to the entity:

```rust,ignore
impl Users {
    fn validate_user(&self, entity: &User) -> Result<(), UserValidationError> {
        if entity.name == "BANNED" {
            return Err(UserValidationError("banned name".into()));
        }
        Ok(())
    }
}
```

### Error propagation

The error appears as a `PostHydrateError` variant on the relevant error type:

| Operation | Error type |
|-----------|-----------|
| `create`, `create_all` | `UserCreateError::PostHydrateError(...)` |
| `find_by_*` | `UserFindError::PostHydrateError(...)` |
| `list_by_*`, `list_for_*`, `find_all` | `UserQueryError::PostHydrateError(...)` |

```rust,ignore
match users.find_by_id(id).await {
    Err(e) if e.was_post_hydrate_error() => {
        println!("entity failed validation: {e}");
    }
    Err(e) => return Err(e.into()),
    Ok(user) => { /* valid */ }
}
```

## Combining both hooks

Both hooks can be used on the same repo. The execution order during `create` and `update` is:

1. Events are persisted to the database
2. `post_persist_hook` runs (async, inside transaction)
3. Entity is hydrated from events
4. `post_hydrate_hook` runs (sync)
5. Entity is returned to the caller

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    post_persist_hook(method = "audit_persist", error = "AuditError"),
    post_hydrate_hook(method = "validate_user", error = "ValidationError"),
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```
