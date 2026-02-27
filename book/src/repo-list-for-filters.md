# fn list_for_filters

The `list_for_filters` function provides a unified interface for querying entities with optional filtering and flexible sorting.
It uses a struct-based API where each filter field is optional, allowing filtering by **N columns simultaneously** — making it ideal for UI table filtering use cases.

The function accepts:

1. A filters struct with `Option<T>` fields (e.g., `UsersFilters { name: Some("Alice".into()), ..Default::default() }`)
2. A sort specification with direction
3. Pagination arguments

When a filter field is `None`, that column is not filtered. When `Some(value)`, only rows matching that value are returned.

## How It Works

For each entity with `list_for` columns, the macro generates:

### A Filters Struct

A `#[derive(Debug, Default)]` struct with one `Option<T>` field per `list_for` column:

```rust,ignore
#[derive(Debug, Default)]
pub struct UserDocumentsFilters {
    pub user_id: Option<UserId>,
    pub status: Option<DocumentStatus>,
}
```

Use `Default::default()` for no filtering, or set specific fields:

```rust,ignore
// No filters - returns all entities
let filters = UserDocumentsFilters::default();

// Filter by user_id only
let filters = UserDocumentsFilters {
    user_id: Some(owner_id),
    ..Default::default()
};

// Filter by both user_id and status
let filters = UserDocumentsFilters {
    user_id: Some(owner_id),
    status: Some(DocumentStatus::Active),
};
```

### Per-Sort-Column Functions

For each `list_by` column, a `list_for_filters_by_{sort_col}` function is generated with SQL that uses nullable WHERE patterns:

```sql
SELECT id FROM user_documents
  WHERE COALESCE(user_id = $1, $1 IS NULL)
    AND COALESCE(status = $2, $2 IS NULL)
    AND (COALESCE(id > $4, true))
  ORDER BY id ASC LIMIT $3
```

When a parameter is `NULL` (i.e., `None`), the `COALESCE` evaluates to `true`, effectively skipping that filter.

### A Dispatch Function

The `list_for_filters` function matches on the sort column and intelligently delegates to the most efficient underlying function:

- **No filters set** (`Filters::default()`): proxies to `list_by_{sort}` (simple query, full index usage)
- **Exactly one filter set**: proxies to `list_for_{col}_by_{sort}` (single-column WHERE, full index usage)
- **Two or more filters set**: uses the per-sort COALESCE-based SQL (multi-column nullable WHERE)

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
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let users = Users { pool: init_pool().await? };

    // No filters - returns all users, sorted by ID
    let all_users = users.list_for_filters(
        UsersFilters::default(),
        Sort {
            by: UsersSortBy::Id,
            direction: ListDirection::Ascending,
        },
        PaginatedQueryArgs {
            first: 10,
            after: None,
        }
    ).await?;

    // Filter by name
    let filtered = users.list_for_filters(
        UsersFilters {
            name: Some("Alice".to_string()),
        },
        Sort {
            by: UsersSortBy::CreatedAt,
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
            UsersFilters {
                name: Some("Alice".to_string()),
            },
            Sort {
                by: UsersSortBy::CreatedAt,
                direction: ListDirection::Descending,
            },
            next_query,
        ).await?;
    }

    Ok(())
}
```
