# Error Types

es-entity uses [errlanes](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/README.md)
as its error library. Every failure a repository returns sits in one of its
lanes: a **rejection** the caller can act on (here, a typed constraint
violation), a **transient** fault worth retrying (a deadlock, a lost
connection), or a **fatal** one that will not succeed on retry (a bug, a
misconfiguration, corrupt stored state). The errlanes guide covers the model
itself — matching on lanes, lifting rejections into domain errors, recording,
retries. This chapter covers what es-entity puts into it.

## The repository error types

```rust,ignore
use es_entity::errlanes::{Fail, Fault, lanes};

pub type RepoReadError = Fault<lanes!(Transient, Fatal)>;
pub type RepoWriteError<C> = Fail<C, lanes!(Transient, Fatal)>;
```

Reads return `RepoReadError`.
Writes return `RepoWriteError<C>`, where `C` is the repository's typed
constraint enum described next.

## Typed constraint violations

`#[derive(EsRepo)]` reads the migration catalog and generates a rejection enum
with one case per constraint on the entity's tables — primary key, unique
indexes (partial and composite included), foreign keys and checks:

```rust,ignore
pub enum UserConstraintViolation {
    Pkey(ConstraintConflict<UserId>),
    EmailKey(ConstraintConflict<String>),
}
```

Variant names are the constraint names with the table prefix stripped
(`users_email_key` → `EmailKey`). Each case carries a `ConstraintConflict<V>`,
where `V` is the column's Rust type — for a composite key, a generated struct
with one field per column:

- `attempted: Option<V>` — the value that collided, taken from the write's own
  input. It is `None` when it cannot be known reliably: a batch write that
  cannot tell which item failed, an opaque CHECK expression, a column type the
  macro does not know. Database message text is never parsed to fill it in.
- `diagnostics` — the table, the exact constraint name, its kind, and the
  original `sqlx::Error`. The enum also offers `constraint_name()`, `kind()`,
  `is_unique()`, `is_foreign_key()` and `is_check()` across all its cases.

Match the case directly:

```rust,ignore
match users.create(new_user).await {
    Err(Fail::Rejected(UserConstraintViolation::EmailKey(conflict))) => {
        // conflict.attempted is Option<String>
    }
    Err(other) => return Err(other.into()),
    Ok(user) => { /* ... */ }
}
```

`attempted` may be personal data. `Display` on a conflict prints only the table
and constraint, never the value or the database's detail text; read `attempted`
deliberately and keep it out of anything that reaches an untrusted client.

To carry a constraint case into your own domain error, see `derive(Lift)` and
`#[errlanes::compose]` in the errlanes guide. A nested repository's constraints
appear on the parent's enum prefixed with the nested field's name
(`OrderConstraintViolation::ItemsSkuKey`).

## What becomes a fault

A few repository outcomes are faults rather than rejections, and they are easy
to expect the other way round:

- **A constraint the catalog does not know** — on a table the repository does
  not own, say — is `Fatal(Invariant)`, with the constraint name as context and
  the `sqlx::Error` as source. The generated enum names every constraint the
  repository can reject on; anything else is a bug in the schema or the query.
- **A required `find_by_*` on a missing row** is `Fatal(Invariant)` with
  `NotFound` as its source. `find_by_*` asserts the row exists. For a key the
  caller supplied, use `maybe_find_by_*` and name the not-found case where the
  caller can see it:

  ```rust,ignore
  let user = users.maybe_find_by_id(id).await?
      .ok_or(RegistrationRejection::UnknownUser { id })?;
  ```

- **An event stream that fails to hydrate** — a missing field, an
  undeserializable event — is `Fatal(CorruptState)`.
- **An optimistic-concurrency conflict** on the events table, including a row
  that vanished between load and `update`, is `Transient(OptimisticConflict)`.
  Every other `sqlx::Error` classifies as errlanes' `classify-sqlx` feature
  describes: deadlocks, serialization failures, pool timeouts and lost
  connections are transient; the rest fatal.

## Classifying a hand-written query

Every generated write op classifies its `sqlx::Error` through `{Entity}WriteError`
— a public, `#[derive(errlanes::Classify)]` type with the same shape as the
private classifier a generated `create`/`update` uses: delegate to the typed
constraint enum when the violation is one the catalog knows, `Transient` on an
events-table conflict, or fall through to errlanes' `classify-sqlx` table. A
query you hand-write against the repository's own tables classifies its error
the same way, with `.classify::<W>()`:

```rust,ignore
use es_entity::errlanes::ClassifyResult;

async fn touch_last_seen(pool: &sqlx::PgPool, id: UserId) -> Result<(), es_entity::RepoWriteError<UserConstraintViolation>> {
    sqlx::query!("UPDATE users SET last_seen_at = now() WHERE id = $1", id as UserId)
        .execute(pool)
        .await
        .classify::<UserWriteError>()?;
    Ok(())
}
```

`UserWriteError`'s own `Rejected`/`Lanes` are inferred from its variants, so
`.classify::<UserWriteError>()?` widens into any `Fail<UserConstraintViolation, L>`
that admits `Transient` and `Fatal` — the same destination a generated write
op returns.
