# fn list_for_filter

The `list_for_filter` function provides a unified interface for querying entities with optional filtering and flexible sorting.
Unlike `list_for` which generates separate functions for each filter/sort combination, `list_for_filter` uses a single function that accepts:

1. A filter enum (e.g., `UsersFilter::WithName` or `UsersFilter::NoFilter`)
2. A sort specification with direction 
3. Pagination arguments

This approach is more flexible when you need to dynamically choose filters and sort orders at runtime, such as in GraphQL resolvers or REST API endpoints where users can specify different combinations of filters and sorting.

## How It Works Internally

The `list_for_filter` function uses pattern matching to delegate to the appropriate underlying function based on the filter and sort combination:

```rust,ignore
pub async fn list_for_filter(
    &self,
    filter: UserDocumentsFilter,
    sort: es_entity::Sort<UserDocumentsSortBy>,
    cursor: es_entity::PaginatedQueryArgs<user_document_cursor::UserDocumentsCursor>,
) -> Result<es_entity::PaginatedQueryRet<UserDocument, user_document_cursor::UserDocumentsCursor>, EsRepoError> {
    let es_entity::Sort { by, direction } = sort;
    let es_entity::PaginatedQueryArgs { first, after } = cursor;

    let res = match (filter, by) {
        // Filter by user_id, sort by ID
        (UserDocumentsFilter::WithUserId(filter_value), UserDocumentsSortBy::Id) => {
            let after = after.map(user_document_cursor::UserDocumentsByIdCursor::try_from).transpose()?;
            let query = es_entity::PaginatedQueryArgs { first, after };
            self.list_for_user_id_by_id(filter_value, query, direction).await?
        }
        // Filter by user_id, sort by created_at
        (UserDocumentsFilter::WithUserId(filter_value), UserDocumentsSortBy::CreatedAt) => {
            let after = after.map(user_document_cursor::UserDocumentsByCreatedAtCursor::try_from).transpose()?;
            let query = es_entity::PaginatedQueryArgs { first, after };
            self.list_for_user_id_by_created_at(filter_value, query, direction).await?
        }
        // No filter, sort by ID
        (UserDocumentsFilter::NoFilter, UserDocumentsSortBy::Id) => {
            let after = after.map(user_document_cursor::UserDocumentsByIdCursor::try_from).transpose()?;
            let query = es_entity::PaginatedQueryArgs { first, after };
            self.list_by_id(query, direction).await?
        }
        // ... more combinations
    };

    Ok(res)
}
```

This pattern matching approach ensures type safety while providing a unified interface for all filter/sort combinations.

## Important Notes

**Cursor and Sort Alignment**: The cursor type in `PaginatedQueryArgs` must match the sort field specified in the `Sort` parameter. If they don't align, you'll get a `CursorDestructureError` at runtime. For example, if you're sorting by `CreatedAt` but your cursor is of type `UsersByIdCursor`, the conversion will fail.

**Column Options**: The available filter and sort combinations are determined by your column configuration:
- **Filters**: Generated for columns with the `list_for` option enabled
- **Sort By**: Generated for columns with the `list_by` option enabled (ID and created_at are included by default)

Only columns configured with these options will appear in the respective filter and sort enums.

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
#         unimplemented!()
#     }
# }
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         unimplemented!()
#     }
# }
# pub struct NewUser { id: UserId, name: String }
# #[derive(EsEntity)]
# pub struct User {
#     pub id: UserId,
#     events: EntityEvents<UserEvent>,
# }
use es_entity::*;

es_entity::entity_id! { UserDocumentId }

# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserDocumentId")]
pub enum UserDocumentEvent {
    Initialized { id: UserDocumentId, owner_id: UserId },
}

# impl IntoEvents<UserDocumentEvent> for NewUserDocument {
#     fn into_events(self) -> EntityEvents<UserDocumentEvent> {
#         EntityEvents::init(
#             self.id,
#             [UserDocumentEvent::Initialized {
#                 id: self.id,
#                 owner_id: self.owner_id,
#             }],
#         )
#     }
# }
# impl TryFromEvents<UserDocumentEvent> for UserDocument {
#     fn try_from_events(events: EntityEvents<UserDocumentEvent>) -> Result<Self, EsEntityError> {
#         Ok(UserDocument { id: events.id().clone(), owner_id: UserId::new(), events })
#     }
# }
pub struct NewUserDocument { id: UserDocumentId, owner_id: UserId }

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
            list_for,
            create(accessor = "owner_id"), update(persist = false)
        )
    )
)]
pub struct UserDocuments {
    pool: sqlx::PgPool
}
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let docs = UserDocuments { pool: init_pool().await? };
    // Assume we have an existing user that we can get the id from
    let owner_id = UserId::new();

    // Batch creating a few entities for illustration
    let new_docs = vec![
        NewUserDocument { id: UserDocumentId::new(), owner_id },
        NewUserDocument { id: UserDocumentId::new(), owner_id }
    ];
    docs.create_all(new_docs).await?;

    // Filter by user_id, sorted by created_at ascending
    let filtered_docs = docs.list_for_filter(
        UserDocumentsFilter::WithUserId(owner_id),
        Sort {
            by: UserDocumentsSortBy::CreatedAt,
            direction: ListDirection::Ascending,
        },
        PaginatedQueryArgs {
            first: 10,
            after: None,
        }
    ).await?;

    assert_eq!(filtered_docs.entities.len(), 2);

    // No filter, sorted by ID descending  
    let all_docs = docs.list_for_filter(
        UserDocumentsFilter::NoFilter,
        Sort {
            by: UserDocumentsSortBy::Id,
            direction: ListDirection::Descending,
        },
        PaginatedQueryArgs {
            first: 10,
            after: None,
        }
    ).await?;

    assert!(all_docs.entities.len() >= 2);

    Ok(())
}
```
