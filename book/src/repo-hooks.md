# Repo Hooks

`EsRepo` supports two optional hooks that run during entity lifecycle operations. Both are configured as attributes on the `#[es_repo(...)]` derive macro.

| Hook | Runs | Signature | Use case |
|------|------|-----------|----------|
| `post_persist_hook` | After events are persisted (inside the transaction) | `async fn(&self, &mut OP, &Entity, LastPersisted<Event>) -> Result<(), E>` | Auditing, side-effect recording, cross-entity writes |
| `post_hydrate_hook` | After an entity is reconstructed from events | `fn(&self, &Entity) -> Result<(), E>` | Validation against external config, policy enforcement |

A hook cannot return a domain rejection — `E` must convert into `errlanes::Fail<core::convert::Infallible>`, so in practice `error` is `errlanes::Fatal` (or `errlanes::Denied` / `errlanes::Transient`, if the hook's failure is one of those lanes instead). A hook failure always widens into whatever `errlanes::Fail<D>` the calling op returns — see [Error Types](./repo-errors.md).

## post_persist_hook

Runs after events have been written to the database but before the entity is returned to the caller. The hook executes inside the same transaction, so it can perform additional database operations or reject the persist.

### Configuration

```rust,ignore
// Simple syntax (error defaults to sqlx::Error, widened via errlanes' classifier):
#[es_repo(entity = "User", post_persist_hook = "on_persist")]

// Explicit syntax with default error:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist"))]

// Explicit syntax with a custom error, which must convert into errlanes::Fail<core::convert::Infallible>:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist", error = "errlanes::Fatal"))]
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
    ) -> Result<(), errlanes::Fatal> {
        // Inspect newly persisted events, write audit records, etc.
        for event in new_events {
            // ...
        }
        Ok(())
    }
}
```

### Which operations run it

The hook runs on every generated operation that persists events: `create`, `create_all`, `update`, `update_all`, soft `delete`, and — for [forgettable](forgettable.md) repos — `forget`. On `forget` it runs after the payload delete and entity rebuild, so it observes the forgotten representation; when an operation persists no events the hook is not invoked.

### Error propagation

When the hook returns an error it widens directly into the op's `errlanes::Fail<UserConstraintViolation>` — as `Fatal` (or whichever lane the hook error carries), not a separate variant:

```rust,ignore
match users.create(new_user).await {
    Err(e) => {
        // e is errlanes::Fail::Fatal(..) if the persist hook rejected
        println!("persist hook failed: {e}");
    }
    Ok(user) => { /* success */ }
}
```

## post_hydrate_hook

Runs synchronously every time an entity is reconstructed from its event stream — on `create`, `create_all`, `find_by_*`, `list_by_*`, `list_for_*`, and `find_all`. It does **not** run on `update` or `delete` since those operate on an already-hydrated entity. This makes it suitable for invariant checks that depend on external state (e.g. configuration or governance rules) rather than the entity's own events.

### Configuration

```rust,ignore
#[es_repo(
    entity = "User",
    post_hydrate_hook(method = "validate_user", error = "errlanes::Fatal")
)]
```

Both `method` and `error` are required; `error` must convert into `errlanes::Fail<core::convert::Infallible>` — see the note above.

### Hook method

The method is synchronous and receives a shared reference to the entity:

```rust,ignore
impl Users {
    fn validate_user(&self, entity: &User) -> Result<(), errlanes::Fatal> {
        if entity.name == "BANNED" {
            return Err(errlanes::Fatal::invariant("banned name"));
        }
        Ok(())
    }
}
```

### Error propagation

The error widens directly into the op's `errlanes::Fail<D>` — as `Fatal` (or whichever lane the hook error carries) — for `create`, `create_all`, `find_by_*`, `list_by_*`, `list_for_*`, and `find_all`:

```rust,ignore
match users.find_by_id(id).await {
    Err(e) if e.lane() == es_entity::errlanes::Lane::Fatal => {
        println!("entity failed validation: {e}");
    }
    Err(e) => return Err(e.into()),
    Ok(user) => { /* valid */ }
}
```

## Combining both hooks

Both hooks can be used on the same repo. During `create`, both hooks run in this order:

1. Events are persisted to the database
2. `post_persist_hook` runs (async, inside transaction)
3. Entity is hydrated from events
4. `post_hydrate_hook` runs (sync)
5. Entity is returned to the caller

During `update`, only `post_persist_hook` runs — no hydration occurs because the entity is already in memory. Similarly, `find_by_*` and `list_*` operations only run `post_hydrate_hook` since they don't persist events.

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    post_persist_hook(method = "audit_persist", error = "errlanes::Fatal"),
    post_hydrate_hook(method = "validate_user", error = "errlanes::Fatal"),
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```
