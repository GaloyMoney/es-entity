mod entities;
mod helpers;

use entities::{user::*, user_document::*};
use es_entity::*;

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "UserDocument",
    columns(user_id(ty = "Option<UserId>", list_for, find_by))
)]
pub struct UserDocuments {
    pool: es_entity::db::Pool,
}

impl UserDocuments {
    pub fn new(pool: es_entity::db::Pool) -> Self {
        Self { pool }
    }
}

/// Regression test: list_for on an Option<T> column with None must return rows
/// where the column IS NULL. Previously the generated SQL used `= ?` which
/// evaluates to NULL (falsy) when the bound value is NULL.
#[tokio::test]
async fn list_for_optional_column_with_none() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let docs = UserDocuments::new(pool);

    // Insert a document with user_id = NULL
    let null_doc = NewUserDocument::builder()
        .id(UserDocumentId::new())
        .user_id(None)
        .build()
        .unwrap();
    docs.create(null_doc).await?;

    // Insert a document with user_id = Some(...)
    let some_user_id = UserId::new();
    let non_null_doc = NewUserDocument::builder()
        .id(UserDocumentId::new())
        .user_id(Some(some_user_id))
        .build()
        .unwrap();
    docs.create(non_null_doc).await?;

    // Query for documents where user_id IS NULL
    let result = docs
        .list_for_user_id_by_id(
            None,
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
            ListDirection::Ascending,
        )
        .await?;

    assert_eq!(
        result.entities.len(),
        1,
        "Expected 1 row with NULL user_id, got {}",
        result.entities.len()
    );
    assert_eq!(result.entities[0].user_id, None);

    // Query for documents where user_id = some_user_id (non-NULL still works)
    let result = docs
        .list_for_user_id_by_id(
            Some(some_user_id),
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
            ListDirection::Ascending,
        )
        .await?;

    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].user_id, Some(some_user_id));

    Ok(())
}

/// Regression test: find_by on an Option<T> column with None must return the
/// row where the column IS NULL.
#[tokio::test]
async fn find_by_optional_column_with_none() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let docs = UserDocuments::new(pool);

    // Insert a document with user_id = NULL
    let null_doc_id = UserDocumentId::new();
    let null_doc = NewUserDocument::builder()
        .id(null_doc_id)
        .user_id(None)
        .build()
        .unwrap();
    docs.create(null_doc).await?;

    // find_by_user_id(None) should find the row
    let found = docs.maybe_find_by_user_id(None).await?;
    assert!(
        found.is_some(),
        "Expected to find document with NULL user_id"
    );
    assert_eq!(found.unwrap().id, null_doc_id);

    Ok(())
}
