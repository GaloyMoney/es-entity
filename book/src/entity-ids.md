# Entity ids

Every entity an `EsRepo` creates gets its id from the repository's *id policy*.
The policy is also the type parameter of the builder that `IntoEvents` receives,
so what a `NewEntity` can do with the id is checked by the compiler.

## The `id` attribute

The id column is configured like any other column, by the repo-level `id`
attribute. `id = "CustomId"` names the type and keeps every default, and the
list form spells the options out:

```rust,ignore
#[es_repo(entity = "Partner", id(ty = "PartnerId", supplied, scope), columns(...))]
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
by `generated(by = "...")`. The `NewEntity` has no id field and reads the id the
repository resolved from the builder:

```rust,ignore
pub struct NewUser {
    pub name: String,
}

impl IntoEvents<UserEvent> for NewUser {
    type Id = Generated;

    fn into_events(self, builder: EntityEventsBuilder<UserEvent, Generated>) -> EntityEvents<UserEvent> {
        let id = builder.id();
        builder.initial_events([UserEvent::Initialized { id, name: self.name }])
    }
}

#[derive(EsRepo)]
#[es_repo(entity = "User")] // same as `id(generated)`
pub struct Users { pool: sqlx::PgPool }
```

A `Generated` `NewEntity` has no route by which a caller-supplied id reaches the
insert: `builder.initial_events` takes no id, the builder cannot be built by
hand, and `EntityEvents::init` is crate-private.

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
well-known constant) opts out with `id(supplied)`. The `NewEntity` brings the
id and passes it to `initial_events`:

```rust,ignore
impl IntoEvents<CustomerEvent> for NewCustomer {
    type Id = Supplied;

    fn into_events(self, builder: EntityEventsBuilder<CustomerEvent, Supplied>) -> EntityEvents<CustomerEvent> {
        builder.initial_events(self.id, [CustomerEvent::Initialized { id: self.id }])
    }
}

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

## A mismatch does not compile

The repo's policy and the `NewEntity`'s `type Id` must agree: a `Supplied`
`NewEntity` used with a default (generated) repo, and the other way round, are
both compile errors.

## Hydration checks the id

After `TryFromEvents` rebuilds an entity, its `id` field is compared with the id
its events are stored under. A mismatch (an `Initialized { id }` payload that
disagrees with the stream) is `Fatal(CorruptState)`.

## Unit tests

Tests build entities without a repository through `es_entity::testing`,
enabled by the `testing` feature. Enable it from `[dev-dependencies]` only, so
production builds do not contain it:

```toml
[dev-dependencies]
es-entity = { version = "...", features = ["testing"] }
```

`use es_entity::testing::*;` brings three functions onto every entity:

| Call | Builds the entity from |
|---|---|
| `User::hydrate(id, [events..])` | any event stream, stored under `id` |
| `User::hydrate_new(id, new_user)` | a `NewUser` whose repo generates the id; the test picks it |
| `User::hydrate_new(new_user)` | a `NewUser` that supplies its own id |

Each returns `Result<User, EntityHydrationError>` and applies the same id check
as a repository load. The events of the returned entity are new (not yet
persisted).
