# Repository errors

Repository writes return `es_entity::RepoWriteError<EntityConstraintViolation>`
and reads return `es_entity::RepoReadError`. These aliases belong to es-entity:

```rust
# extern crate es_entity;
use es_entity::errlanes::{Fail, Fault, lanes};

pub type RepoReadError = Fault<lanes!(Transient, Fatal)>;
pub type RepoWriteError<C> = Fail<C, lanes!(Transient, Fatal)>;
```

Both enable Transient and Fatal, never Denied.
A write may return a structured, caller-correctable rejection; a read cannot.
Both carriers preserve the standard lane markers and their original sources.

## Typed constraint cases

An `EsRepo` derives a rejection enum with one case per recognized database
constraint. For example:

```rust,ignore
pub enum UserConstraintViolation {
    Pkey(ConstraintConflict<UserId>),
    EmailKey(ConstraintConflict<String>),
}
```

There is no separate discriminator enum, `Own` wrapper, or `Unknown` rejection.
Match the case directly. Constraint names strip the table prefix and are
converted to Rust variant names; named indexes keep their exact identity.
Primary keys, unique indexes (including partial and composite indexes), foreign
keys, and checks come from the migration catalog. Conventional primary/unique
names are retained for repositories without discoverable migrations.

`ConstraintConflict<V>` contains `attempted: Option<V>` and structured diagnostics
(table, exact constraint name, kind, and original SQLx source). Single writes
capture typed values from the actual input when available. Composite keys use a
generated struct with named fields for all key columns. Batch failures leave the
attempted value absent when the failing item cannot be identified reliably.
Opaque CHECK expressions and unknown column types also leave values absent.
Database message text is never parsed to manufacture typed attempted values.

Values may be sensitive. Default Display prints only table/constraint identity,
not attempted values or database detail text. Deliberate consumers may inspect
`attempted`. Generated `diagnostics()`, `constraint_name()`, `kind()`,
`is_unique()`, `is_foreign_key()`, and `is_check()` provide diagnostic access;
typed variant matching is the domain mapping API.

```rust,ignore
match repo.create(new_user).await {
    Err(Fail::Rejected(UserConstraintViolation::EmailKey(conflict))) => {
        // conflict.attempted is Option<String>; do not replace absence with "".
    }
    other => { /* normal propagation or handling */ }
}
```

Known constraints are Rejected. Unknown database constraints become
Fatal(Invariant) at the repository boundary with their original SQLx source and
constraint identity. NOT NULL/exclusion failures retain the central SQL
classifier's fatal treatment. Optimistic event-sequence conflicts remain
Transient(OptimisticConflict). Duplicate IDs during creation are Rejected even
when PostgreSQL reports the events-table constraint first. A vanished row during
an update is a transient race; a torn create batch is a fatal invariant.

## Domain mapping and composition

Use one lifting framework for exhaustive forwarding and explicit partial domain
mapping:

```rust,ignore
#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(UserConstraintViolation, unhandled = fatal)]
pub enum RegistrationRejection {
    #[error("email already exists: {0}")]
    #[rejection(code = "EMAIL_ALREADY_EXISTS")]
    #[lift(UserConstraintViolation::EmailKey)]
    EmailAlreadyExists(ConstraintConflict<String>),
}

use errlanes::LiftResult;
async fn register(...) -> Result<User, Fail<RegistrationRejection, errlanes::lanes!(Transient, Fatal)>> {
    Ok(repo.create(new_user).await.lift()?)
}
```

Unhandled **known** cases become Fatal(Invariant) at this domain boundary, with
the original rejection as source. This is separate from unknown constraints at
the repository boundary. Partial mode never creates an infallible
`From<UserConstraintViolation>` for the domain rejection. Use it only for cases
that truly indicate a violated domain invariant; legitimate alternatives must
remain rejections.

`derive(Lift)` generates conversions; `derive(Rejection)` generates codes and
levels. Omitting `unhandled = fatal` selects strict mode. Name every case with
`#[lift(Source::Variant)]`; omitted cases fail compilation. Matching fields
forward automatically, and a total `From<Source>` enables `.widen()?` on a
failure result or `?` on a bare rejection. Forwarded metadata preserves the leaf
code and severity by default; an explicit code denotes a domain reinterpretation.
Neither derive implements the other's trait. `Lift` can also be used without
`Rejection` when only conversion is needed.

To import an entire family without repeating its cases:

```rust,ignore
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum RegistrationRejection {
    #[compose(flatten)]
    User(UserConstraintViolation),
    #[error("registration closed")]
    Closed,
}
```

`compose` supplies both `Rejection` and `Lift`; only unrelated derives need to
be listed. Each source case becomes `UserCase`, using the placeholder name as
its prefix. Codes and levels stay those of the source. Explicit strict/partial
`#[lift(...)]` mappings from other families can coexist with whole-family
imports, and explicit lifts provide custom or unprefixed destination names.

The placeholder disappears. Nested repositories use the same schema protocol,
with the nested field name as prefix: `OrderConstraintViolation::ItemsSkuKey`
contains the child's typed conflict directly. There is no nested discriminator
tree. `FamilySchema` must be reexported alongside `Family` when a composing
consumer uses a reexport or renamed source. The three-crate fixture covers both
ordinary families and generated nested repository writes.

## Read semantics and faults

`maybe_find_by_*` returns `Ok(None)` for absence. A required `find_by_*` assumes
the row exists and returns Fatal(Invariant) with `NotFound` as source when it is
missing. For a caller-supplied key that may legitimately be absent, use an
optional read and define the caller-facing not-found rejection at the domain
boundary.

```rust,ignore
let user = repo.maybe_find_by_id(id).await?
    .ok_or(UserRejection::NotFound { id })?;
```

Hydration/stored-data failures remain Fatal(CorruptState). Denial cannot enter
ordinary repository signatures: custom hooks must convert into the declared
profile, and a hook that can deny does not satisfy that bound. Default SQLx
hooks convert into `RepoReadError` or `RepoWriteError<C>`.

Fatal payloads are diagnostic data for operators and tests. Production code
handles the lane uniformly: stop, surface, and alert at the owning boundary.
Do not branch on a fatal source to recover a legitimate rejection. Retry only
when the transaction boundary proves it is safe; a transient classification does
not remove commit ambiguity or change batch-isolation retry policy.

See the [errlanes guide](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/README.md)
for subset matching, conversion inference, composition metadata, and settlement.
