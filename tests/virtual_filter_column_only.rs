mod entities;
mod helpers;

use std::collections::HashSet;

use entities::vf_account::*;
use es_entity::*;
use sqlx::PgPool;

/// Repo with *only* a virtual filter column — no physical `list_for` column
/// at all. Exercises the emission-gating change: `list_for_filters*` must
/// still be generated, and the dispatch proxy must still check the virtual
/// filter before falling back to plain `list_by` (never silently degrading
/// to it just because there is no physical `list_for` column to justify the
/// unified path).
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "VfAccount",
    columns(flagged(
        ty = "bool",
        virtual = "EXISTS (SELECT 1 FROM vf_account_flags f WHERE f.account_id = vf_accounts.id)",
        list_for
    ),)
)]
pub struct VfAccountsFlagOnly {
    pool: PgPool,
}

impl VfAccountsFlagOnly {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_account(repo: &VfAccountsFlagOnly, status: &str) -> anyhow::Result<VfAccount> {
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
async fn virtual_only_none_returns_both() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsFlagOnly::new(pool.clone());
    let unique_status = format!("vf_only_none_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters { flagged: None },
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
    assert!(ids.contains(&flagged_account.id));
    assert!(ids.contains(&plain_account.id));

    Ok(())
}

#[tokio::test]
async fn virtual_only_true_returns_only_flagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsFlagOnly::new(pool.clone());
    let unique_status = format!("vf_only_true_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                flagged: Some(true),
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
    assert!(ids.contains(&flagged_account.id));
    assert!(!ids.contains(&plain_account.id));

    Ok(())
}

#[tokio::test]
async fn virtual_only_false_returns_only_unflagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsFlagOnly::new(pool.clone());
    let unique_status = format!("vf_only_false_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                flagged: Some(false),
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
    assert!(!ids.contains(&flagged_account.id));
    assert!(ids.contains(&plain_account.id));

    Ok(())
}
