# Batch lookups

When a page of results needs related entities for every row (a GraphQL dataloader, for example), calling a lookup once per row costs one round trip per row. The repo generates two batch primitives that answer the whole page in a single query:

| Function | Keyed by | Returns |
|---|---|---|
| `find_all` | entity id | `HashMap<EntityId, Entity>` |
| `find_all_for_<column>s` | a `list_for` column | `HashMap<ColumnTy, Vec<Entity>>` |

`find_all` is the `1:1` lookup. `find_all_for_<column>s` is its `1:n` sibling: it takes a slice of column values and returns every entity whose column matches one of them.

```rust,ignore
fn find_all_for_user_ids<Out: From<Entity>>(
    &self,
    user_ids: &[UserId],
    direction: es_entity::ListDirection,
) -> Result<HashMap<UserId, Vec<Out>>, es_entity::RepoFault>
fn find_all_for_user_ids_in_op<'a, Out: From<Entity>>(
    &self,
    op: impl IntoOneTimeExecutor<'a>,
    user_ids: &[UserId],
    direction: es_entity::ListDirection,
) -> Result<HashMap<UserId, Vec<Out>>, es_entity::RepoFault>
```

## Which columns get one

A `list_for` column gets a `find_all_for_<column>s` function only when it opts in with the `find_all_for` flag next to its `by(..)` columns. Without the flag the repo expands exactly as it would without this feature.

```rust,ignore
user_id(ty = "UserId", list_for(by(created_at), find_all_for))
user_id(
    ty = "UserId",
    list_for(by(created_at), find_all_for),
    update(persist = false, accessor = "values().user_id")
)
```

The flag is an error, naming the column, on:

- `Option<T>` and `nullable` columns, since `= ANY` can never match a `NULL` key
- `Forgettable<T>` columns
- `virtual` columns
- a `list_for` without a `by(..)` column, since the `by(..)` column orders each group

Opting in adds two requirements that `list_for` alone does not have:

- `ty` must implement `Hash + Eq + Clone`, because it keys the returned map.
- The key must be readable from the entity (see [Key](#semantics)).

The name is the column name pluralised (`account_holder_id` becomes `find_all_for_account_holder_ids`, `currency` becomes `find_all_for_currencies`). A repo with `delete = "soft"` also gets the `_include_deleted` variants, and an `in_op_only` repo only gets the `_in_op` functions.

A column declared with several `by(..)` columns gets one function, ordered by the first one.

## Semantics

- **One statement.** The whole key slice is answered by a single `WHERE <column> = ANY($1)` query. There is no page size and no cursor, because this is a lookup and not a listing.
- **Absent keys stay absent.** A key with no matching entity is not in the map, exactly like `find_all`. If a consumer needs an empty `Vec` for every requested key, it fills the map itself with `entry(key).or_default()`.
- **Order.** Inside each `Vec` the entities follow the first `by(..)` column in the given `ListDirection`, with `id` as the tiebreak, the same order `list_for_<column>_by_<by>` would return for that single key.
- **Soft delete.** Deleted entities are hidden, as in `list_for`. The `_include_deleted` variants return them as well.
- **Empty slice.** An empty key slice returns an empty map.
- **Key.** The key of each entity is read from the hydrated entity with the column's `update(accessor = "<expr>")`, the same accessor a `list_by` cursor uses, and it applies even with `update(persist = false)`. Without one, the key is the entity field named after the column. `create(accessor = ..)` is never used, because it reads the `New*` struct, not the entity. The key is cloned, so its type must implement `Hash + Eq + Clone`. The slice type must also be usable as a Postgres array bind (`sqlx::postgres::PgHasArrayType`).

## Scoped repositories

On a [scoped repo](./scoped-repositories.md) the functions take the scope as their first argument, and the generated `Scoped{Repo}` view has matching delegates that capture it. The scope adds its usual conjunct next to the `ANY` predicate, so rows from another scope behave like missing rows.

If the lookup column is also the scope column (an `Owner`-style scope on the same column), the scope is redundant: when the scope value is in the key list the result is the same as without it, and when it is not, the result is empty.

## Example

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate tokio;
# extern crate anyhow;
# extern crate uuid;
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! { UserId }
use es_entity::*;

es_entity::entity_id! { UserDocumentId }

# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserDocumentId")]
pub enum UserDocumentEvent {
    Initialized { id: UserDocumentId, owner_id: UserId },
}

# impl IntoEvents<UserDocumentEvent> for NewUserDocument {
#     type IdSource = FromRepo;
#     fn into_events(self, builder: EntityEventsBuilder<UserDocumentEvent, FromRepo>) -> EntityEvents<UserDocumentEvent> {
#         let id = builder.id();
#         builder.initial_events([UserDocumentEvent::Initialized {
#             id,
#             owner_id: self.owner_id,
#         }])
#     }
# }
# impl TryFromEvents<UserDocumentEvent> for UserDocument {
#     fn try_from_events(events: EntityEvents<UserDocumentEvent>) -> Result<Self, EntityHydrationError> {
#         let mut h = UserDocument::hydrator();
#         for event in events.iter_all() {
#             match event {
#                 UserDocumentEvent::Initialized { id, owner_id } => {
#                     h.id(*id).owner_id(*owner_id);
#                 }
#             }
#         }
#         h.build(events)
#     }
# }
pub struct NewUserDocument { owner_id: UserId }

#[derive(EsEntity)]
pub struct UserDocument {
    pub id: UserDocumentId,
    owner_id: UserId,
    events: EntityEvents<UserDocumentEvent>,
}

#[derive(EsRepo)]
#[es_repo(
    entity = "UserDocument",
    columns(
        user_id(
            ty = "UserId",
            list_for(by(created_at), find_all_for),
            create(accessor = "owner_id"),
            update(persist = false, accessor = "owner_id")
        )
    )
)]
pub struct UserDocuments {
    pool: sqlx::PgPool
}
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = std::env::var("PG_CON").unwrap_or_else(|_| "postgres://user:password@localhost:5432/pg".to_string());
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let docs = UserDocuments { pool: init_pool().await? };
    let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());

    docs.create_all(vec![
        NewUserDocument { owner_id: alice },
        NewUserDocument { owner_id: alice },
        NewUserDocument { owner_id: bob },
    ])
    .await?;

    // One query for all three users
    let found: std::collections::HashMap<UserId, Vec<UserDocument>> = docs
        .find_all_for_user_ids(&[alice, bob, carol], ListDirection::Descending)
        .await?;

    assert_eq!(found[&alice].len(), 2);
    assert_eq!(found[&bob].len(), 1);
    // Carol owns nothing, so she is absent rather than mapped to an empty `Vec`
    assert!(!found.contains_key(&carol));

    Ok(())
}
```
