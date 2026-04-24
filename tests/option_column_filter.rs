mod entities;
mod helpers;

use entities::task::*;
use es_entity::*;
use sqlx::PgPool;

/// Repo with two list_for columns:
/// - workspace_id: Option<WorkspaceId>, list_for by(id) — paired with id sort
/// - status: String, list_for by(created_at) — NOT paired with id sort
///
/// When sorting by id, the dispatch logic has:
/// - Both None -> list_by_id
/// - workspace_id=Some, status=None -> list_for_workspace_id_by_id (paired)
/// - workspace_id=None, status=Some -> list_for_filters_by_id (fallback)
/// - Both Some -> list_for_filters_by_id (fallback)
///
/// The fallback path uses `IS NOT DISTINCT FROM` for each filter column.
/// Before the fix, it used COALESCE which matched ALL rows when a filter was NULL.
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

/// Regression test: when the multi-filter fallback SQL is reached with a None
/// filter value for an Option column, it should match only NULL rows for that
/// column (IS NOT DISTINCT FROM NULL), not ALL rows (COALESCE bug).
#[tokio::test]
async fn list_for_filters_fallback_none_matches_only_null_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();
    // Use a unique status to isolate from other test runs
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

    // Create task WITHOUT workspace_id (NULL), different status
    let _task_null_other = tasks
        .create(
            NewTask::builder()
                .id(TaskId::new())
                .status("other")
                .build()
                .unwrap(),
        )
        .await?;

    // Filter: workspace_id=None, status=Some(unique_status)
    // This hits the fallback path (status is unpaired with id sort).
    // With the fix: workspace_id IS NOT DISTINCT FROM NULL → matches only NULL rows
    // Before fix: COALESCE(workspace_id = NULL, NULL IS NULL) → TRUE for ALL rows
    let result = tasks
        .list_for_filters(
            TaskFilters {
                workspace_id: None,
                status: Some(unique_status.clone()),
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

    // Should match ONLY the task with NULL workspace_id AND matching status
    assert_eq!(
        result.entities.len(),
        1,
        "Expected 1 task (null workspace + status), got {}",
        result.entities.len()
    );
    assert_eq!(result.entities[0].id, task_null_ws.id);

    // Verify task_with_ws is NOT included (has non-NULL workspace_id)
    assert!(
        result.entities.iter().all(|t| t.id != task_with_ws.id),
        "Task with non-NULL workspace_id should NOT match a NULL filter"
    );

    Ok(())
}

/// Test that filtering with Some(value) on an Option column matches only
/// rows with that specific value (not NULL rows).
#[tokio::test]
async fn list_for_filters_fallback_some_excludes_null_rows() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let tasks = Tasks::new(pool);

    let ws_id = WorkspaceId::new();
    // Use a unique status to isolate from other test runs
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
    // Both filters set → hits the fallback path
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

    // Should match ONLY the task with matching workspace_id AND status
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].id, task_with_ws.id);

    // NULL workspace task should NOT be included
    assert!(result.entities.iter().all(|t| t.id != task_null_ws.id));

    Ok(())
}
