# fn list_for_filters

The `list_for_filters` function provides a unified interface for querying entities with optional filtering and flexible sorting.
It uses a struct-based API where each filter field is optional, allowing filtering by **N columns simultaneously** — making it ideal for UI table filtering use cases.

The function accepts:

1. A filters struct with `Option<T>` fields (e.g., `UserFilters { name: Some("Alice".into()), ..Default::default() }`)
2. A sort specification with direction
3. Pagination arguments

When a filter field is `None`, that column is not filtered. When `Some(value)`, only rows matching that value are returned.

## How It Works

For each entity with `list_for` columns, the macro generates:

### A Filters Struct

A `#[derive(Debug, Default)]` struct with one `Option<T>` field per `list_for` column:

```rust,ignore
#[derive(Debug, Default)]
pub struct UserDocumentFilters {
    pub user_id: Option<UserId>,
    pub status: Option<DocumentStatus>,
}
```

Use `Default::default()` for no filtering, or set specific fields:

```rust,ignore
// No filters - returns all entities
let filters = UserDocumentFilters::default();

// Filter by user_id only
let filters = UserDocumentFilters {
    user_id: Some(owner_id),
    ..Default::default()
};

// Filter by both user_id and status
let filters = UserDocumentFilters {
    user_id: Some(owner_id),
    status: Some(DocumentStatus::Active),
};
```

### Per-Sort-Column Functions

For each `list_by` column, a `list_for_filters_by_{sort_col}` function is generated. Instead of one catch-all query, it contains one static SQL query per **filter combination × cursor state** and dispatches at runtime on which filters are `Some`. Present filters compile to plain, index-friendly (sargable) predicates; absent filters are omitted entirely:

```sql
-- user_id = Some(..), status = None, first page
SELECT id FROM user_documents
  WHERE user_id = $1
  ORDER BY id ASC LIMIT $2

-- both filters set, paginating from a cursor
SELECT id FROM user_documents
  WHERE user_id = $1 AND status = $2 AND (id > $4)
  ORDER BY id ASC LIMIT $3
```

This matters for performance: the planner can turn `col = $k` into an index condition, which is impossible through a `COALESCE(col = $k, $k IS NULL)` catch-all (a single generic plan must serve both `NULL` and non-`NULL` parameters, so the predicate never becomes an index qual and every call full-scans the table).

For entities with more than 4 `list_for` columns the combination matrix is capped: only the no-filter, single-filter, and all-filters combinations get specialized queries, and remaining combinations fall back to the catch-all COALESCE-based SQL (correct, just not sargable):

```sql
SELECT id FROM user_documents
  WHERE COALESCE(user_id = $1, $1 IS NULL)
    AND COALESCE(status = $2, $2 IS NULL)
    AND (COALESCE(id > $4, true))
  ORDER BY id ASC LIMIT $3
```

#### Compile-time cost — `sargable_filters` is opt-in

The specialized matrix emits one `sqlx::query!` per filter combination × cursor state × direction. For an entity with N `list_for` columns that grows as 2^N (3^N with optional columns), and across many repos this can add thousands of compile-time-checked queries — a large release-codegen tax. The matrix is therefore **off by default** and the catch-all COALESCE query above is used for the multi-filter case.

To opt in per repo (only for entities whose multi-filter list queries are hot paths that benefit from index usage), set `sargable_filters` on the `#[es_repo(...)]` attribute:

```rust,ignore
#[es_repo(
    entity = "Transfer",
    columns(
        account_id(ty = "AccountId", list_for(by(created_at))),
        status(ty = "String", list_for(by(created_at))),
    ),
    sargable_filters,
)]
pub struct Transfers { pool: PgPool }
```

Single-filter (`list_for_{col}_by_{sort}`) and no-filter (`list_by_{sort}`) queries are always sargable and cheap (O(N)), so they are generated regardless of this flag. Only the multi-filter combination matrix is gated.

### A Dispatch Function

The `list_for_filters` function matches on the sort column and intelligently delegates to the most efficient underlying function:

- **No filters set** (`Filters::default()`): proxies to `list_by_{sort}` (simple query, full index usage)
- **Exactly one filter set**: proxies to `list_for_{col}_by_{sort}` (single-column WHERE, full index usage)
- **Two or more filters set**: uses the per-sort specialized query matching the exact filter combination (sargable), falling back to the COALESCE-based SQL only for combinations beyond the specialization cap

## Important Notes

**Cursor and Sort Alignment**: The cursor type in `PaginatedQueryArgs` must match the sort field specified in the `Sort` parameter.

**Column Options**: Filter fields are generated for columns with the `list_for` option. Sort options are generated for columns with `list_by` (ID and created_at are included by default).

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
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
# }
# impl IntoEvents<UserEvent> for NewUser {
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         EntityEvents::init(
#             self.id,
#             [UserEvent::Initialized { id: self.id, name: self.name }],
#         )
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
#         let mut name = String::new();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { name: n, .. } => { name = n.clone(); }
#                 UserEvent::NameUpdated { name: n } => { name = n.clone(); }
#             }
#         }
#         Ok(User { id: events.id().clone(), name, events })
#     }
# }
# pub struct NewUser { id: UserId, name: String }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     pub name: String,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

#[derive(EsRepo)]
#[es_repo(
    entity = "User",
    columns(
        name(ty = "String", list_for(by(id, created_at)))
    )
)]
pub struct Users {
    pool: sqlx::PgPool
}
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = std::env::var("PG_CON").unwrap_or_else(|_| "postgres://user:password@localhost:5432/pg".to_string());
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let users = Users { pool: init_pool().await? };

    // No filters - returns all users, sorted by ID
    let all_users = users.list_for_filters(
        UserFilters::default(),
        Sort {
            by: UserSortBy::Id,
            direction: ListDirection::Ascending,
        },
        PaginatedQueryArgs {
            first: 10,
            after: None,
        }
    ).await?;

    // Filter by name
    let filtered = users.list_for_filters(
        UserFilters {
            name: Some("Alice".to_string()),
        },
        Sort {
            by: UserSortBy::CreatedAt,
            direction: ListDirection::Descending,
        },
        PaginatedQueryArgs {
            first: 10,
            after: None,
        }
    ).await?;

    // Paginate through results
    if let Some(next_query) = filtered.into_next_query() {
        let next_page = users.list_for_filters(
            UserFilters {
                name: Some("Alice".to_string()),
            },
            Sort {
                by: UserSortBy::CreatedAt,
                direction: ListDirection::Descending,
            },
            next_query,
        ).await?;
    }

    Ok(())
}
```
