# id

Every entity an `EsRepo` creates gets its id from the repository's *id source*:
the repo itself, or the caller. The source is also the type parameter of the
builder that `IntoEvents` receives (see [New Entity](./new-entity.md)), so what
a `NewEntity` can do with the id is checked by the compiler.

## The `id` attribute

The id column is configured like any other column, by the repo-level `id`
attribute. `id = "CustomId"` names the type and keeps every default, and the
list form spells the options out:

```rust,ignore
#[es_repo(entity = "Partner", id(ty = "PartnerId", source = caller, scope), columns(...))]
```

| option | meaning |
| --- | --- |
| `ty = "..."` | the id type; defaults to `{Entity}Id` |
| `source = repo` | the repository generates the id (the default) |
| `generator = "path"` | the repository generates the id with `path` |
| `source = caller` | the caller puts the id in the `NewEntity` |
| `scope`, `scope(variant = "...")` | the id is the repo's scope column, see [Scoped repositories](./scoped-repositories.md) |

It differs from an ordinary column in three ways: `ty` is optional, `find_by`
and `list_by` are always on, and only the id takes a `source`. The id is never
declared inside `columns(...)`.

## Source: the repo (the default)

By default the repository generates the id: `<IdType>::new()`, or the function
named by `generator = "..."`. The `NewEntity` declares
`type IdSource = FromRepo` and reads the id from the builder:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "User")] // same as `id(source = repo)`
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
#[es_repo(entity = "PublicId", id(generator = "Self::next_id"))]
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

## Source: the caller

An entity with a deterministic id (derived from another aggregate, a
well-known constant) takes it from the caller with `id(source = caller)`. Its
`NewEntity` declares `type IdSource = FromCaller`, carries the id and passes it
to `initial_events`:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "Customer", id(source = caller))]
pub struct Customers { pool: sqlx::PgPool }
```

An id from the caller is a uniqueness claim, so `Pkey` stays a case of the
constraint violation. A repo that names its id type explicitly writes
`id(ty = "CustomId", source = caller)`. `generator` needs `source = repo`.

The repository checks that the id the `NewEntity` handed to `initial_events`
is the one it inserts under; if they differ the create fails with
`Fatal(Invariant)`.

For entities that may already exist, the id is one of the keys of
[`create_or_find_in_op`](./repo-create-or-find.md).

## A mismatch does not compile

The repo's `source` and the `NewEntity`'s `type IdSource` must agree: a
`FromCaller` `NewEntity` used with a default (repo-sourced) repo, and the other
way round, are both compile errors.

## Hydration checks the id

After `TryFromEvents` rebuilds an entity, its `id` field is compared with the id
its events are stored under. A mismatch (an `Initialized { id }` payload that
disagrees with the stream) is `Fatal(CorruptState)`.
