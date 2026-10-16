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
- **`Found`**: the `new` entity's unique keys (id included) match exactly one
  stored entity, which is returned unchanged. `new`'s events are dropped and the
  post-persist hook does not run. Only keys are compared: Ensure never updates.
- **`Rejected(KeysSpanEntities { ids, keys })`**: the keys match more than one
  stored entity, for example the id is held by one and the code by another. The
  rejection carries the ids and key names, never values. The caller's
  transaction is not aborted.
- **`Transient`**: under `REPEATABLE READ` / `SERIALIZABLE` the row committed
  by another transaction is not visible, so retry in a new transaction. Under
  `READ COMMITTED` it is found.

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
WHERE id = $1 OR code = $2 OR (ledger = $3 AND currency = $4)
```

Values are bind parameters, read from the `NewEntity` the way `create` reads
them. A `None` optional key never matches.

## Keys

The keys are the id plus every unique constraint or non-partial unique index
the migrations declare, on columns the create writes as-is. A table without a
schema catalog is ensured by id alone. A repo whose only key is the id returns
`RepoFault`, since its keys cannot span entities.

A conflict on a partial or expression index whose columns are not also a full
unique key cannot be looked up, so it ends in `Transient(OptimisticConflict)`.

Scoped and soft-deleting repos get no method. The lookup cannot name a scope,
and a soft-deleted row still holds its key but is invisible to the lookup.

`<Entity>EnsureRejection` also keeps the repo's check-constraint members and
nested children's rejections. Its own code is `<table>_keys_span_entities`.
