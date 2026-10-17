# create_or_find_in_op

For entities that may already exist, the repo gets `create_or_find_in_op`. It
creates the entity, or returns the stored one when the `NewEntity`'s unique
keys already name it.

```rust,ignore
pub enum Ensured<E> { Created(E), Found(E) }

pub async fn create_or_find_in_op<OP>(&self, op: &mut OP, new: NewAccount)
    -> Result<Ensured<Account>, es_entity::RepoWriteError<AccountEnsureRejection>>
```

## Outcomes

- **`Created`**: the insert hit no unique constraint. It behaves like `create_in_op`.
- **`Found`**: the `new` entity's unique keys match exactly one stored entity,
  which is returned unchanged. `new`'s events are dropped and the
  post-persist hook does not run. Only keys are compared: Ensure never updates.
- **`Rejected(KeysSpanEntities { ids, keys })`**: the keys match more than one
  stored entity, for example the code is held by one and the email by another. The
  rejection carries the ids and key names, never values. The caller's
  transaction is not aborted.
- **`Rejected(KeysHeldByDeleted { ids, keys })`**: soft-delete repos only, when
  no live entity but only soft-deleted ones hold a key.
- **`Rejected(<constraint member>)`**: the insert conflicted on a unique
  arbiter the lookup cannot see, see [Other arbiters](#other-arbiters).
- **`Transient`**: only under `REPEATABLE READ` / `SERIALIZABLE`, where the
  row committed by another transaction is not visible: retry in a new
  transaction. Under `READ COMMITTED` it is found.

> **Take the id from the returned entity.** The stored entity's id may differ
> from `new`'s id (it was found through another key). If "found by id is fine,
> a foreign code is an error" is what you need, compare the ids yourself or use
> `create_in_op`.

## How it works

The insert is `ON CONFLICT DO NOTHING` without a target, so every unique
constraint, unique index, partial unique index and exclusion constraint
arbitrates. An uncommitted conflicting row blocks the insert until its
transaction ends. If no row was inserted, a second `SELECT` looks the entity up
by every key at once:

```sql
SELECT id, .. FROM accounts
WHERE code = $1 OR (ledger = $2 AND currency = $3)
```

Values are bind parameters, read from the `NewEntity` the way `create` reads
them. A `None` optional key never matches.

## Keys

The keys are every unique constraint or non-partial unique index the
migrations declare, on columns the create writes as-is. With
`id(source = caller)` the id is a key too, and comes first. A repo-generated id
is fresh, so it is never looked up, and a repo with repo-generated ids and no
other key gets no method. A table without a schema catalog has no keys besides
a caller-chosen id. A repo with a single key returns `RepoFault`, since its
keys cannot span entities.

`<Entity>EnsureRejection` also keeps the repo's check-constraint members, the
members of unique constraints that are not keys, and nested children's
rejections. Its own code is `<table>_keys_span_entities`. A repo whose table
declares a unique constraint that is not a key returns
`RepoWriteError<..EnsureRejection>` even with a single key.

## Other arbiters

The insert arbitrates on every unique constraint and index, but the lookup only
covers the keys. A conflict on an arbiter that is not a key, such as a partial
or expression unique index, or a unique constraint on a column the create does
not write as-is, leaves the lookup empty. The call then retries the plain
insert inside a savepoint and reports the conflict as `Rejected` with the
constraint's member, the same member `create_in_op` rejects with (without the
attempted value). The savepoint is rolled back, so the caller's transaction
stays usable. A conflict on a key itself at that point is the snapshot
isolation case above, `Transient(OptimisticConflict)`. A constraint the
generated enum does not know is `Fatal(Invariant)`, naming the constraint.

## Scoped repos

A [scoped repo](./scoped-repositories.md) keeps the unscoped signature, like
`create`. The lookup also reads the scope columns of every match and compares
them with the `new` entity's. A match in another scope rejects with
`KeysHeldOutsideScope { keys }` (code `<table>_keys_held_outside_scope`). It
carries key names only, never the other scope's ids, and it wins over any match
inside the scope.
