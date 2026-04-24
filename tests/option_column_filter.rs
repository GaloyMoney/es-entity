mod entities;
mod helpers;

use entities::task::*;
use es_entity::*;
use sqlx::PgPool;

/// Repo with two list_for columns:
/// - workspace_id: Option<WorkspaceId>, list_for by(id) — paired with id sort
/// - status: String, list_for by(created_at) — NOT paired with id sort
///
/// The individual list_for_workspace_id_by_id method uses
/// `IS NOT DISTINCT FROM` for the Option column, so passing None
/// correctly matches only NULL rows.
///
/// For list_for_filters, `Option<Option<WorkspaceId>>` has three cases:
/// - None (outer)        → don't filter → match ALL rows
/// - Some(Some(ws_id))   → filter by specific value
/// - Some(None)          → filter by NULL rows only
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Task",
    columns(
        workspace_id(ty = "Option<WorkspaceId>", list_for),
        status(ty = "String", list_for(by(created_at)))
    )
)]
pub struct Tasks {
    pool: PgPool,
}

impl Tasks {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Test: list_for_filters with workspace_id=None means "don't filter by
/// workspace_id" — the COALESCE pattern correctly skips the filter and
/// returns all rows matching the status filter.
#[tokio::test]
async fn list_for_filters_none_skips_filter() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();
    let unique_status = format!("active_{}", TaskId::new());

    // Create task WITH workspace_id
    let task_with_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .workspace_id(ws_id)
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Create task WITHOUT workspace_id (NULL)
    let task_null_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Filter: workspace_id=None (skip filter), status=Some(unique_status)
    // Should return BOTH tasks — None means "don't filter by workspace_id"
    let result = tasks
        .list_for_filters(
            TaskFilters {
                workspace_id: None,
                status: Some(unique_status),
            },
            Sort {
                by: TaskSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;

    assert_eq!(
        result.entities.len(),
        2,
        "Expected both tasks (None means skip filter), got {}",
        result.entities.len()
    );
    let ids: Vec<_> = result.entities.iter().map(|t| t.id).collect();
    assert!(ids.contains(&task_with_ws.id));
    assert!(ids.contains(&task_null_ws.id));

    Ok(())
}

/// Test: list_for_filters with workspace_id=Some(Some(value)) filters
/// by that specific value, excluding NULL rows.
#[tokio::test]
async fn list_for_filters_some_value_filters_correctly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();
    let unique_status = format!("pending_{}", TaskId::new());

    // Create task WITH workspace_id
    let task_with_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .workspace_id(ws_id)
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Create task WITHOUT workspace_id (NULL)
    let task_null_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Filter: workspace_id=Some(Some(ws_id)), status=Some(unique_status)
    let result = tasks
        .list_for_filters(
            TaskFilters {
                workspace_id: Some(Some(ws_id)),
                status: Some(unique_status),
            },
            Sort {
                by: TaskSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;

    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].id, task_with_ws.id);
    assert!(result.entities.iter().all(|t| t.id != task_null_ws.id));

    Ok(())
}

/// Regression test: list_for_workspace_id_by_id(None) should match only
/// NULL rows. Before the fix, `WHERE workspace_id = $1` with $1=NULL
/// produced `workspace_id = NULL` which is always NULL/FALSE in SQL.
/// Now uses `IS NOT DISTINCT FROM` for Option columns.
#[tokio::test]
async fn list_for_column_none_matches_only_null_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();

    // Create task WITH workspace_id
    let task_with_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .workspace_id(ws_id)
                .status("any")
                .build()
                .unwrap(),
        )
        .await?;

    // Create task WITHOUT workspace_id (NULL)
    let task_null_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status("any")
                .build()
                .unwrap(),
        )
        .await?;

    // Call list_for_workspace_id_by_id directly with None
    // Before fix: WHERE workspace_id = NULL → no matches
    // After fix: WHERE workspace_id IS NOT DISTINCT FROM NULL → matches NULL rows
    let result = tasks
        .list_for_workspace_id_by_id(
            None::<WorkspaceId>,
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Ascending,
        )
        .await?;

    assert!(
        result.entities.iter().any(|t| t.id == task_null_ws.id),
        "Task with NULL workspace_id should match a None filter"
    );
    assert!(
        result.entities.iter().all(|t| t.id != task_with_ws.id),
        "Task with non-NULL workspace_id should NOT match a None filter"
    );

    Ok(())
}

/// Test: list_for_workspace_id_by_id(Some(ws_id)) matches only rows
/// with that specific value (not NULL rows).
#[tokio::test]
async fn list_for_column_some_excludes_null_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();

    // Create task WITH workspace_id
    let task_with_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .workspace_id(ws_id)
                .status("any")
                .build()
                .unwrap(),
        )
        .await?;

    // Create task WITHOUT workspace_id (NULL)
    let task_null_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status("any")
                .build()
                .unwrap(),
        )
        .await?;

    let result = tasks
        .list_for_workspace_id_by_id(
            Some(ws_id),
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
            ListDirection::Ascending,
        )
        .await?;

    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].id, task_with_ws.id);
    assert!(result.entities.iter().all(|t| t.id != task_null_ws.id));

    Ok(())
}

/// Test: list_for_filters with workspace_id=Some(None) filters to only
/// NULL rows — this is the third case of Option<Option<T>> where the
/// caller explicitly wants rows where the column IS NULL.
#[tokio::test]
async fn list_for_filters_some_none_matches_only_null_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();
    let unique_status = format!("null_filter_{}", TaskId::new());

    // Create task WITH workspace_id
    let _task_with_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .workspace_id(ws_id)
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Create task WITHOUT workspace_id (NULL)
    let task_null_ws = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status(&unique_status)
                .build()
                .unwrap(),
        )
        .await?;

    // Filter: workspace_id=Some(None) means "filter by NULL workspace_id"
    let result = tasks
        .list_for_filters(
            TaskFilters {
                workspace_id: Some(None),
                status: Some(unique_status),
            },
            Sort {
                by: TaskSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;

    assert_eq!(
        result.entities.len(),
        1,
        "Expected only the NULL-workspace task, got {}",
        result.entities.len()
    );
    assert_eq!(result.entities[0].id, task_null_ws.id);
    assert!(
        result.entities.iter().all(|t| t.id != _task_with_ws.id),
        "Task with non-NULL workspace_id should NOT match a Some(None) filter"
    );

    Ok(())
}
