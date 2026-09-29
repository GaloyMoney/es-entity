# Repo Hooks

`EsRepo` supports two optional hooks that run during entity lifecycle operations. Both are configured as attributes on the `#[es_repo(...)]` derive macro.

| Hook | Runs | Signature | Use case |
|------|------|-----------|----------|
| `post_persist_hook` | After events are persisted (inside the transaction) | `async fn(&self, &mut OP, &Entity, LastPersisted<Event>) -> Result<(), E>` | Auditing, side-effect recording, cross-entity writes |
| `post_hydrate_hook` | After an entity is reconstructed from events | `fn(&self, &Entity) -> Result<(), E>` | Validation against external config, policy enforcement |

A hook cannot return a domain rejection. `post_persist_hook` only ever runs from a write path, so its `E` must satisfy `errlanes::Fail<{Entity}ConstraintViolation>: From<E>` directly. `post_hydrate_hook` runs from both writes (`create`/`create_all`) and pure reads (`find_by_*`, `list_by_*`, ...), so its `E` must satisfy the narrower `errlanes::Fault: From<E>` instead — which then widens into whichever of `Fail<D>`/`Fault` the calling op returns. In practice `error` is `errlanes::Fatal` for both (or `errlanes::Denied` / `errlanes::Transient`, if the hook's failure is one of those lanes instead) — see [`Fail` vs `Fault`](./repo-errors.md#fail-vs-fault). A custom `error = X` is preserved as the resulting `Fatal`'s `source` — visible to operators via `exception.type`/`exception.message` on the span, and downcastable in a test that wants to assert exactly which hook failure fired (see [Error Types](./repo-errors.md) on why production code never needs to).

### What a hook may fail with

Hooks cannot reject. `post_persist_hook` failures are infrastructure: a `sqlx::Error` classifies into `Transient` (a lost race, safe to retry) or `Fatal` (broken invariant), the same way any other database error does. A `post_hydrate_hook` failing on read describes stored state — `Fatal(CorruptState)` / `Fatal(Config)` — or policy (`Denied`); it is never a caller-correctable outcome, because a read cannot reject. Validation of caller input is a `Rejection` and belongs in the command or the `New{Entity}` builder, before the repo is ever called.

## post_persist_hook

Runs after events have been written to the database but before the entity is returned to the caller. The hook executes inside the same transaction, so it can perform additional database operations or reject the persist.

### Configuration

```rust,ignore
// Simple syntax (error defaults to sqlx::Error, widened via errlanes' classifier):
#[es_repo(entity = "User", post_persist_hook = "on_persist")]

// Explicit syntax with default error:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist"))]

// Explicit syntax with a custom error, which must convert into errlanes::Fail<UserConstraintViolation>:
#[es_repo(entity = "User", post_persist_hook(method = "on_persist", error = "errlanes::Fatal"))]
```

### Hook method

The method must be defined on the repo struct with this signature. With the default `error` (omitted, or `sqlx::Error`), the hook just runs a query through `op` — an outbox or audit write — and lets `?` propagate; errlanes' classifier turns any `sqlx::Error` into `Transient`/`Fatal` the same way the rest of the op's own queries do, so a hook author never picks a lane for an infra failure:

```rust,ignore
impl Users {
    async fn on_persist<OP: es_entity::AtomicOperation>(
        &self,
        op: &mut OP,
        entity: &User,
        new_events: es_entity::events::LastPersisted<'_, UserEvent>,
    ) -> Result<(), sqlx::Error> {
        for event in new_events {
            sqlx::query!(
                "INSERT INTO user_outbox (user_id, event_type) VALUES ($1, $2)",
                entity.id as UserId,
                event.event.event_type(),
            )
            .execute(op.as_executor())
            .await?;
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
    post_hydrate_hook(method = "validate_user", error = "errlanes::Fault")
)]
```

Both `method` and `error` are required; `error` must convert into `errlanes::Fault` — see the note above.

### Hook method

The method is synchronous and receives a shared reference to the entity. A read cannot reject, so a failure here always describes stored state or policy — never caller input (see [What a hook may fail with](#what-a-hook-may-fail-with) above):

```rust,ignore
impl Users {
    fn validate_user(&self, entity: &User) -> Result<(), errlanes::Fault> {
        if entity.config_schema_version > CURRENT_SCHEMA_VERSION {
            return Err(errlanes::Fatal::new(errlanes::FatalKind::Config)
                .with_context("user config schema v1 loaded under v2")
                .into());
        }
        Ok(())
    }
}
```

### Error propagation

The error widens directly into whatever the calling op returns — `errlanes::Fail<D>` on `create`/`create_all`, `errlanes::Fault` on `find_by_*`/`list_by_*`/`list_for_*`/`find_all` — as `Fatal` (or whichever lane the hook error carries):

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
    post_hydrate_hook(method = "validate_user", error = "errlanes::Fault"),
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```
