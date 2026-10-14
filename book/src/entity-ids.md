# Entity ids

Every entity an `EsRepo` creates gets its id from the repository's *id policy*.
The policy is also the type parameter of the builder that `IntoEvents` receives,
so what a `NewEntity` can do with the id is checked by the compiler.

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
#[es_repo(entity = "User")] // same as `id = generated`
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
#[es_repo(entity = "PublicId", id = generated(by = "Self::next_id"))]
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
well-known constant) opts out with `id = supplied`. The `NewEntity` brings the
id and passes it to `initial_events`:

```rust,ignore
impl IntoEvents<CustomerEvent> for NewCustomer {
    type Id = Supplied;

    fn into_events(self, builder: EntityEventsBuilder<CustomerEvent, Supplied>) -> EntityEvents<CustomerEvent> {
        builder.initial_events(self.id, [CustomerEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo)]
#[es_repo(entity = "Customer", id = supplied)]
pub struct Customers { pool: sqlx::PgPool }
```

A supplied id is a uniqueness claim made by the caller, so `Pkey` stays a case of
the constraint violation. A repo that names its id type explicitly writes it as
`id_type = "CustomId"` next to `id = supplied`.

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

Repositories hand `into_events` its builder. A unit test that wants the initial
events of a `NewEntity` builds one itself with the hidden constructors
`EntityEventsBuilder::__generated(id)` / `EntityEventsBuilder::__supplied()`.
