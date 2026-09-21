mod entities;
mod helpers;

use std::collections::HashSet;

use entities::vf_account::*;
use es_entity::*;
use sqlx::PgPool;

/// A value virtual whose `ty` is a strongly-typed `entity_id!` (not a bare
/// scalar like `i64`/`String`) compiles and binds — proving the value
/// virtual genuinely reuses the physical `list_for` binding path (which
/// already supports custom id types) rather than a scalar-only shortcut.
/// Separate file from `tests/virtual_filter_value.rs` since both declare a
/// repo over `entity = "VfAccount"`, and the generated `VfAccountFilters`
/// struct would otherwise collide at module scope.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "VfAccount",
    columns(flagged_by(
        ty = "VfAccountId",
        virtual = "EXISTS (SELECT 1 FROM vf_account_flags f WHERE f.account_id = vf_accounts.id AND f.account_id = {value})",
        list_for
    ),)
)]
pub struct VfAccountsFlaggedBy {
    pool: PgPool,
}

impl VfAccountsFlaggedBy {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_account(repo: &VfAccountsFlaggedBy, status: &str) -> anyhow::Result<VfAccount> {
    Ok(repo
        .create(
            NewVfAccount::builder()
                .id(VfAccountId::new())
                .status(status)
                .build()
                .unwrap(),
        )
        .await?)
}

async fn flag(pool: &PgPool, account_id: VfAccountId) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO vf_account_flags (account_id, flag) VALUES ($1, 'due')")
        .bind(account_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn value_virtual_with_entity_id_ty_compiles_and_binds() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsFlaggedBy::new(pool.clone());
    let unique_status = format!("vfv_idty_{}", VfAccountId::new());

    let account = create_account(&accounts, &unique_status).await?;
    flag(&pool, account.id).await?;
    let other_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, other_account.id).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                flagged_by: Some(account.id),
            },
            Sort {
                by: VfAccountSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 100,
                after: None,
            },
        )
        .await?;

    let ids: HashSet<_> = result.entities().iter().map(|e| e.id).collect();
    assert_eq!(ids, HashSet::from([account.id]));
    assert!(!ids.contains(&other_account.id));

    Ok(())
}
