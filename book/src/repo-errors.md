# Error Types

Every generated repo op returns `errlanes`' four-lane model: `Rejected(D)` (a typed domain outcome, caller-correctable), `Denied` (authorization), `Transient` (retry the same call), or `Fatal` (a broken invariant or infrastructure failure — page an operator). `EsRepo` is "born-classified": it picks the lane for you, so a caller never has to sniff a `sqlx::Error` to know whether something is worth retrying.

### `Fail` vs `Fault`

A write can reject: `create`/`create_all`/`update`/`update_all`/`forget`/`delete` return `Result<T, errlanes::Fail<{Entity}ConstraintViolation>>`, where `Fail<D>` is the full four-arm view (`Rejected(D)`/`Denied`/`Transient`/`Fatal`). A read cannot: `find_by_*`/`maybe_find_by_*`/`find_all`/`list_by_*`/`list_for_*` return `Result<T, errlanes::Fault>`, where `Fault` is `Fail` minus the `Rejected` arm — the type itself says a read never hands back a domain outcome. `?` widens a `Fault` into any `Fail<D>` for free, so calling a read from inside a write path needs no `map_err`:

```rust,ignore
async fn rename(&self, id: UserId, name: String) -> Result<(), errlanes::Fail<UserConstraintViolation>> {
    let mut user = self.find_by_id(id).await?; // Fault widens into Fail<UserConstraintViolation>
    user.rename(name);
    self.update(&mut user).await?;
    Ok(())
}
```

For an entity called `User`, the macro produces one domain rejection type and two supporting enums:

| Type | Description |
|------|-------------|
| `UserColumn` | Enum of indexed columns (e.g. `Id`, `Name`, `Email`) |
| `UserConstraint` | Enum of the table's known constraints (unique / foreign key / check), plus `Unknown` |
| `UserConstraintViolation` | The repo's one `Rejection` — returned as `errlanes::Fail::Rejected` from `create`/`create_all`/`update`/`update_all`/`forget`/`delete` |

## `UserConstraintViolation`

```rust,ignore
pub struct UserConstraintViolation {
    constraint: Option<UserConstraint>,
    constraint_name: Option<String>,
    column: Option<UserColumn>,
    value: Option<String>,
}
```

When a `create`, `create_all`, `update`, `update_all`, or `forget` operation violates a **unique**, **foreign key**, or **check** constraint, the error comes back as `errlanes::Fail::Rejected(UserConstraintViolation { .. })`. (`NOT NULL` and exclusion violations are not classified and surface as `Fatal` instead — they indicate a programming error, not a caller-correctable domain conflict.) For unique violations, `column()` identifies which column caused the violation and `value()` contains the conflicting value extracted from the PostgreSQL error detail. For foreign key and check violations — or unique constraints not recognized as belonging to one of the entity's columns — `column()` and `value()` are `None`; use `constraint()` (or the raw `constraint_name()`) to identify the constraint instead.

`UserConstraintViolation` implements `errlanes::Rejection` (`Code = UserConstraint`) and `errlanes::Liftable` (`Key = UserConstraint`) — the trait a domain rejection's `#[rejection(lift = UserConstraintViolation)]` reads to lift a constraint into one of its own variants (see the `errlanes` crate docs).

> **Security note:** `value()` contains attacker-influenced input that was rejected by a unique constraint and is frequently PII (e.g. an email address). Do not propagate it to untrusted API clients — a caller can probe which values already exist (user enumeration) — and be aware it may end up in logs via the error's `Display`/`Debug` output. `Display` never prints it; at trust boundaries, prefer matching on `constraint()` / `column()` and map the error to a neutral client-facing message.

```rust,ignore
let result = users.create(new_user).await;
match result {
    Ok(user) => { /* success */ }
    Err(errlanes::Fail::Rejected(cv)) if cv.column() == Some(UserColumn::Email) => {
        let value = cv.value(); // Option<&str>
        println!("email {} already taken", value.unwrap_or("unknown"));
    }
    Err(e) => return Err(e.into()),
}
```

### Typed constraints: `UserConstraint`

Alongside the column enum, the macro derives a `UserConstraint` enum with one variant per constraint on the entity's table known at compile time: the declared columns' unique constraints (convention names like `users_email_key` / `users_pkey`) plus every unique, foreign key, and check constraint discoverable from the migrations directory (the same catalog that drives `list_for_filters` specialization). Variant names strip the table prefix — `entries_account_not_account_set_fkey` on table `entries` becomes `EntryConstraint::AccountNotAccountSetFkey`.

This makes dispatching on a hand-written foreign-key or check constraint typo-proof — no string matching at the call site, and a renamed constraint in a migration surfaces as a compile error instead of a silently dead match arm:

```rust,ignore
match result {
    Ok(entry) => { /* success */ }
    Err(errlanes::Fail::Rejected(cv))
        if cv.constraint() == Some(EntryConstraint::AccountNotAccountSetFkey) =>
    {
        return Err(AppError::EntryTargetsAccountSet);
    }
    Err(e) => return Err(e.into()),
}
```

Each variant knows its raw name (`constraint.name()` / `Display`) and kind (`constraint.kind()` → `ConstraintKind::{Unique, ForeignKey, Check}`). Constraints created outside discoverable migrations can't be typed and report `UserConstraint::Unknown`; fall back to `constraint_name()` for those:

```rust,ignore
Err(errlanes::Fail::Rejected(cv)) if cv.constraint_name() == Some("added_at_runtime_fkey") => { /* ... */ }
```

The macro maps PostgreSQL constraint names to columns automatically. It uses the convention `{table}_{column}_key` for unique constraints and `{table}_pkey` for the primary key, and additionally derives the real names of any **named** single-column unique index from your migrations (the same index catalog that drives `list_for_filters` specialization — see [list_for_filters](./repo-list-for-filters.md)). So a `CREATE UNIQUE INDEX idx_unique_email ON users (email)` in a migration is mapped to the `email` column with no extra annotation — as long as the migrations directory is discoverable (crate-local `migrations/`, an ancestor `migrations/` up to the repo root, or `ES_ENTITY_MIGRATIONS_DIR`). A composite `UNIQUE (a, b)` is mapped to its **last** key column (`b`) — the discriminating column, with the leading columns acting as its scope — so a `UNIQUE (partner_id, name)` violation reports the `name` column.

### Nested entity errors

For aggregates with nested entities (e.g. `Order` containing `OrderItem`s), `OrderConstraintViolation` is an enum instead of a struct: an `Own { .. }` variant for the parent's own violations, plus one tuple variant per nested child wrapping that child's own `{Child}ConstraintViolation`. A child violation widens into the parent's via the generated `From` impl, so nested write paths only need `.map_err(errlanes::Fail::widen)`:

```rust,ignore
match err {
    errlanes::Fail::Rejected(OrderConstraintViolation::OrderItems(item_cv))
        if item_cv.column() == Some(OrderItemColumn::Sku) =>
    {
        let val = item_cv.value();
    }
    _ => return Err(err.into()),
}
```

`constraint()`, `constraint_name()`, `column()`, and `value()` on the parent only report the parent's own violations (`Own { .. }`); they return `None` for a nested variant — match the nested variant directly to inspect it, as above.

## Concurrent modification

When optimistic concurrency control detects a conflict (duplicate event sequence) on `update`, `update_all`, `forget`, or `delete`, that is not a domain rejection — it is `errlanes::Fail::Transient(..)`, because the correct response is "reload and retry", not "ask the caller to fix their input". Wrap the call in `errlanes::retry` to have this handled automatically, or match the lane directly:

```rust,ignore
match users.update(&mut user).await {
    Err(e) if e.is_transient() => {
        // reload and retry
    }
    Err(e) => return Err(e.into()),
    Ok(()) => {}
}
```

## Not found

`find_by_*` returns `Result<Entity, errlanes::Fault>`: a missing row is `Fatal(Invariant)`, not a rejection — by calling `find_by_*` instead of `maybe_find_by_*`, the caller has already asserted the row must exist, so its absence is a broken invariant, not something the caller is meant to branch on. Its source is a `NotFound` carrying the entity name, the column searched, and the value that was not found (see [es_query](./es-query.md)).

Use `maybe_find_by_*` to get `Ok(None)` instead of an error when the entity legitimately may not exist:

```rust,ignore
match users.maybe_find_by_id(some_id).await? {
    Some(user) => { /* found */ }
    None => { /* not found — an expected outcome, not an error */ }
}
```

## Hooks

`PostPersistHookError` and `PostHydrateError` surface as `Fatal` — see [Hooks](./repo-hooks.md) for how they're wrapped.
