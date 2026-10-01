# Repo Hooks

`EsRepo` supports two optional hooks that run during entity lifecycle operations. Both are configured as attributes on the `#[es_repo(...)]` derive macro.

| Hook | Runs | Signature | Use case |
|------|------|-----------|----------|
| `post_persist_hook` | After events are persisted (inside the transaction) | `async fn(&self, &mut OP, &Entity, LastPersisted<Event>) -> Result<(), Fault<lanes!(Transient, Fatal)>>` | Auditing, outbox writes, cross-entity writes |
| `post_hydrate_hook` | After an entity is reconstructed from events | `fn(&self, &Entity) -> Result<(), Fatal>` | Invariant checks against external configuration |

## Hook errors

Each hook's error type is the narrowest one that covers what it can do (see [Error Types](./repo-errors.md)); there is nothing to configure.

- `post_persist_hook` runs queries through `op`, so it returns `errlanes::Fault<lanes!(Transient, Fatal)>` — the same fault type a read returns. Database calls propagate with `?`: a `sqlx::Error` converts into it through errlanes' classifier, so a deadlock becomes `Transient` and a constraint violation `Fatal`, exactly as it would for the operation's own queries. Anything else is wrapped explicitly, choosing the lane: `Fatal::from_error(FatalKind::Config, e)` or `Transient::new(kind).with_source(e)`.
- `post_hydrate_hook` is synchronous and has no `op`, so nothing it does can fail transiently. It returns a bare `errlanes::Fatal`: a stored entity that violates an external invariant will violate it on the next read too.

A hook cannot reject or deny. Caller-input validation and authorization belong before the repository call; a hook's failure describes stored state or infrastructure, which the caller cannot correct.

## post_persist_hook

Runs after events have been written to the database but before the entity is returned to the caller. The hook executes inside the same transaction, so it can perform additional database operations or fail the persist.

### Configuration

```rust,ignore
#[es_repo(entity = "User", post_persist_hook = "on_persist")]
```

### Hook method

The method is defined on the repo struct. A typical hook runs a query through `op` — an outbox or audit write — and lets `?` propagate:

```rust,ignore
impl Users {
    async fn on_persist<OP: es_entity::AtomicOperation>(
        &self,
        op: &mut OP,
        entity: &User,
        new_events: es_entity::LastPersisted<'_, UserEvent>,
    ) -> Result<(), errlanes::Fault<errlanes::lanes!(Transient, Fatal)>> {
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

A hook failure is the operation's result — `Fail::Fatal(..)` or `Fail::Transient(..)` on `create`, for example — not a separate variant. A standalone operation's transaction rolls back with it; inside a caller's `op`, the caller decides.

```rust,ignore
match users.create(new_user).await {
    Err(Fail::Fatal(fatal)) => println!("persist failed: {fatal}"),
    Err(other) => return Err(other.into()),
    Ok(user) => { /* success */ }
}
```

## post_hydrate_hook

Runs synchronously every time an entity is reconstructed from its event stream — on `create`, `create_all`, `find_by_*`, `list_by_*`, `list_for_*`, and `find_all`. It does **not** run on `update` or `delete` since those operate on an already-hydrated entity. This makes it suitable for invariant checks that depend on external state (e.g. configuration or governance rules) rather than the entity's own events.

### Configuration

```rust,ignore
#[es_repo(entity = "User", post_hydrate_hook = "validate_user")]
```

### Hook method

The method is synchronous and receives a shared reference to the entity:

```rust,ignore
impl Users {
    fn validate_user(&self, entity: &User) -> Result<(), errlanes::Fatal> {
        if entity.config_schema_version > CURRENT_SCHEMA_VERSION {
            return Err(errlanes::Fatal::from_error(
                errlanes::FatalKind::Config,
                SchemaVersionError { found: entity.config_schema_version },
            ));
        }
        Ok(())
    }
}
```

### Error propagation

The fault is the calling operation's result — `RepoWriteError<C>` on `create`/`create_all`, `RepoReadError` on the read operations. Nothing marks it as having come from the hook; a hook that wants to be recognisable gives its `Fatal` a distinct source type, which a caller can then find in the error's source chain:

```rust,ignore
use std::error::Error as _;

match users.find_by_id(id).await {
    Err(Fault::Fatal(fatal)) if fatal.source().is_some_and(|s| s.is::<SchemaVersionError>()) => {
        println!("entity failed validation: {fatal}");
    }
    Err(other) => return Err(other.into()),
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
    post_persist_hook = "audit_persist",
    post_hydrate_hook = "validate_user",
)]
pub struct Users {
    pool: sqlx::PgPool,
}
```
