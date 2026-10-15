# id

Every entity an `EsRepo` creates gets its id from the repository's *id policy*.
The policy is also the type parameter of the builder that `IntoEvents` receives
(see [New Entity](./new-entity.md)), so what a `NewEntity` can do with the id is
checked by the compiler.

## The `id` attribute

The id column is configured like any other column, by the repo-level `id`
attribute. `id = "CustomId"` names the type and keeps every default, and the
list form spells the options out:

```rust,ignore
#[es_repo(entity = "Partner", id(ty = "PartnerId", scope), columns(...))]
```

| option | meaning |
| --- | --- |
| `ty = "..."` | the id type; defaults to `{Entity}Id` |
| `generated` | the repository mints the id (the default) |
| `generated(by = "path")` | the repository mints the id with `path` |
| `supplied` | the `NewEntity` brings the id |
| `scope`, `scope(variant = "...")` | the id is the repo's scope column, see [Scoped repositories](./scoped-repositories.md) |

It differs from an ordinary column in three ways: `ty` is optional, `find_by`
and `list_by` are always on, and only the id takes a policy (`generated` or
`supplied`). The id is never declared inside `columns(...)`.

## Generated ids (the default)

By default the repository mints the id: `<IdType>::new()`, or the function named
by `generated(by = "...")`. The `NewEntity` declares `type Id = Generated` and
reads the id from the builder:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "User")] // same as `id(generated)`
pub struct Users { pool: sqlx::PgPool }
```

Because a collision of a freshly generated id is a fault rather than something a
caller can act on, the primary key is **not** a case of
`{Entity}ConstraintViolation` for such a repo. When nothing else can reject (no
unique column, no check constraint, nothing nested) the create functions return
the plain `Result<Entity, RepoFault>`.

To draw the id from somewhere else, name an async function taking the repo and
the operation:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "PublicId", id(generated(by = "Self::next_id")))]
pub struct PublicIds { pool: sqlx::PgPool }

impl PublicIds {
    async fn next_id<OP: AtomicOperation + ?Sized>(
        &self,
        op: &mut OP,
    ) -> Result<PublicIdId, RepoFault> {
        // e.g. `SELECT nextval(..)`
    }
}
```

## Supplied ids

An entity with a deterministic id (derived from another aggregate, a
well-known constant) opts out with `id(supplied)`. Its `NewEntity` declares
`type Id = Supplied` and passes its own id to `initial_events`:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "Customer", id(supplied))]
pub struct Customers { pool: sqlx::PgPool }
```

A supplied id is a uniqueness claim made by the caller, so `Pkey` stays a case of
the constraint violation. A repo that names its id type explicitly writes
`id(ty = "CustomId", supplied)`.

The repository checks that the id the `NewEntity` handed to `initial_events`
is the one it inserts under; if they differ the create fails with
`Fatal(Invariant)`.

A supplied-id repo also gets
[`create_or_find_by_id_in_op`](./repo-create.md#create_or_find_by_id_in_op), for
entities that may already exist.

## A mismatch does not compile

The repo's policy and the `NewEntity`'s `type Id` must agree: a `Supplied`
`NewEntity` used with a default (generated) repo, and the other way round, are
both compile errors.
