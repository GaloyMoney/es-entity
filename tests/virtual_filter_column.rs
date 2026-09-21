mod entities;
mod helpers;

use std::collections::HashSet;

use entities::vf_account::*;
use es_entity::*;
use sqlx::PgPool;

/// Repo with one physical `list_for` column (`status`) and one virtual
/// filter column (`flagged`): a SQL predicate correlated against a side
/// table (`vf_account_flags`) that this repo does not otherwise own —
/// standing in for the lana-bank motivating case (a credit facility filtered
/// by whether it has a *due* obligation, a fact that lives on another
/// entity's table).
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "VfAccount",
    columns(
        status(ty = "String", list_for(by(created_at))),
        flagged(
            ty = "bool",
            virtual = "EXISTS (SELECT 1 FROM vf_account_flags f WHERE f.account_id = vf_accounts.id)",
            list_for
        ),
    )
)]
pub struct VfAccounts {
    pool: PgPool,
}

impl VfAccounts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_account(repo: &VfAccounts, status: &str) -> anyhow::Result<VfAccount> {
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

/// Case 1: `flagged: None` applies no conjunct at all — both flagged and
/// unflagged rows come back.
#[tokio::test]
async fn virtual_filter_none_returns_both() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccounts::new(pool.clone());
    let unique_status = format!("vf_none_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
                flagged: None,
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
    assert!(ids.contains(&plain_account.id));

    Ok(())
}

/// Case 2: `flagged: Some(true)` — only rows with a matching flags row.
#[tokio::test]
async fn virtual_filter_true_returns_only_flagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccounts::new(pool.clone());
    let unique_status = format!("vf_true_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
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

/// Case 3: `flagged: Some(false)` — only rows with *no* matching flags row.
#[tokio::test]
async fn virtual_filter_false_returns_only_unflagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccounts::new(pool.clone());
    let unique_status = format!("vf_false_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
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

/// Case 4: `status` and `flagged` compose — the intersection, checked under
/// both sort directions.
#[tokio::test]
async fn virtual_filter_composes_with_physical_filter() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccounts::new(pool.clone());
    let status_a = format!("vf_combo_a_{}", VfAccountId::new());
    let status_b = format!("vf_combo_b_{}", VfAccountId::new());

    let a_flagged = create_account(&accounts, &status_a).await?;
    flag(&pool, a_flagged.id).await?;
    let a_plain = create_account(&accounts, &status_a).await?;
    let b_flagged = create_account(&accounts, &status_b).await?;
    flag(&pool, b_flagged.id).await?;

    for direction in [ListDirection::Ascending, ListDirection::Descending] {
        let result = accounts
            .list_for_filters(
                VfAccountFilters {
                    status: Some(status_a.clone()),
                    flagged: Some(true),
                },
                Sort {
                    by: VfAccountSortBy::Id,
                    direction,
                },
                PaginatedQueryArgs {
                    first: 100,
                    after: None,
                },
            )
            .await?;

        let ids: HashSet<_> = result.entities().iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            HashSet::from([a_flagged.id]),
            "direction={direction:?}"
        );
        assert!(!ids.contains(&a_plain.id));
        assert!(!ids.contains(&b_flagged.id));
    }

    Ok(())
}

/// Case 5: pagination under a virtual filter walks every matching row
/// exactly once, in cursor order, terminating with `has_next_page = false`.
#[tokio::test]
async fn virtual_filter_paginates_correctly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccounts::new(pool.clone());
    let unique_status = format!("vf_page_{}", VfAccountId::new());

    let mut flagged_ids = Vec::new();
    for _ in 0..5 {
        let account = create_account(&accounts, &unique_status).await?;
        flag(&pool, account.id).await?;
        flagged_ids.push(account.id);
    }
    for _ in 0..5 {
        create_account(&accounts, &unique_status).await?;
    }

    let mut seen = Vec::new();
    let mut query = PaginatedQueryArgs {
        first: 2,
        after: None,
    };
    loop {
        let page = accounts
            .list_for_filters(
                VfAccountFilters {
                    status: Some(unique_status.clone()),
                    flagged: Some(true),
                },
                Sort {
                    by: VfAccountSortBy::Id,
                    direction: ListDirection::Ascending,
                },
                query,
            )
            .await?;
        let has_next_page = page.has_next_page;
        seen.extend(page.entities().iter().map(|e| e.id));
        match page.into_next_query() {
            Some(next) => {
                assert!(has_next_page);
                query = next;
            }
            None => {
                assert!(!has_next_page);
                break;
            }
        }
    }

    let seen_set: HashSet<_> = seen.iter().copied().collect();
    let expected_set: HashSet<_> = flagged_ids.iter().copied().collect();
    assert_eq!(seen_set, expected_set);
    assert_eq!(seen.len(), flagged_ids.len(), "no id repeated across pages");

    Ok(())
}
