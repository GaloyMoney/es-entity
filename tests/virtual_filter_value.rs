mod entities;
mod helpers;

use std::collections::HashSet;

use entities::vf_account::*;
use es_entity::*;
use sqlx::PgPool;

/// Repo with one physical `list_for` column (`status`), one bool-polarity
/// virtual (`flagged`, unchanged from `tests/virtual_filter_column.rs`), and
/// one *value* virtual (`min_flags`): a SQL predicate whose `{value}`
/// placeholder is rewritten to a bound `$k` parameter — the parameterized
/// extension this test file exercises. Reuses the same `vf_accounts` /
/// `vf_account_flags` tables #236 already migrated; no new migration.
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
        min_flags(
            ty = "i64",
            virtual = "(SELECT COUNT(*) FROM vf_account_flags f WHERE f.account_id = vf_accounts.id) >= {value}",
            list_for
        ),
    )
)]
pub struct VfAccountsWithValue {
    pool: PgPool,
}

impl VfAccountsWithValue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn create_account(repo: &VfAccountsWithValue, status: &str) -> anyhow::Result<VfAccount> {
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

/// Case 1: `min_flags: None` applies no conjunct — both a flagged and an
/// unflagged account come back.
#[tokio::test]
async fn value_virtual_none_returns_both() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsWithValue::new(pool.clone());
    let unique_status = format!("vfv_none_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
                flagged: None,
                min_flags: None,
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

/// Case 2: `min_flags: Some(1)` — only accounts with at least one flags row.
#[tokio::test]
async fn value_virtual_some_one_returns_only_flagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsWithValue::new(pool.clone());
    let unique_status = format!("vfv_one_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;
    let plain_account = create_account(&accounts, &unique_status).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
                flagged: None,
                min_flags: Some(1),
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

/// Case 3: `min_flags: Some(2)` — only an account flagged twice; a
/// once-flagged account is excluded. Proves the bound value is genuinely
/// compared, not merely "has any flag".
#[tokio::test]
async fn value_virtual_some_two_excludes_once_flagged() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsWithValue::new(pool.clone());
    let unique_status = format!("vfv_two_{}", VfAccountId::new());

    let twice_flagged = create_account(&accounts, &unique_status).await?;
    flag(&pool, twice_flagged.id).await?;
    flag(&pool, twice_flagged.id).await?;
    let once_flagged = create_account(&accounts, &unique_status).await?;
    flag(&pool, once_flagged.id).await?;

    let result = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
                flagged: None,
                min_flags: Some(2),
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
    assert_eq!(ids, HashSet::from([twice_flagged.id]));
    assert!(!ids.contains(&once_flagged.id));

    Ok(())
}

/// Case 4: the two virtual dimensions compose — `min_flags: Some(1)` ANDed
/// with `flagged: Some(false)` is a contradiction (an account cannot have
/// zero matching rows in the `EXISTS` sense and simultaneously satisfy
/// `count >= 1`) and returns nothing; ANDed with `flagged: Some(true)` it
/// returns the flagged account.
#[tokio::test]
async fn value_virtual_composes_with_bool_virtual() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsWithValue::new(pool.clone());
    let unique_status = format!("vfv_compose_{}", VfAccountId::new());

    let flagged_account = create_account(&accounts, &unique_status).await?;
    flag(&pool, flagged_account.id).await?;

    let contradiction = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status.clone()),
                flagged: Some(false),
                min_flags: Some(1),
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
    assert!(contradiction.entities().is_empty());

    let composed = accounts
        .list_for_filters(
            VfAccountFilters {
                status: Some(unique_status),
                flagged: Some(true),
                min_flags: Some(1),
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
    let ids: HashSet<_> = composed.entities().iter().map(|e| e.id).collect();
    assert_eq!(ids, HashSet::from([flagged_account.id]));

    Ok(())
}

/// Case 5: pagination under a value-virtual filter walks every qualifying
/// row exactly once, in cursor order — the value predicate must still apply
/// on every page, including the cursor-gated ones.
#[tokio::test]
async fn value_virtual_paginates_correctly() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let accounts = VfAccountsWithValue::new(pool.clone());
    let unique_status = format!("vfv_page_{}", VfAccountId::new());

    let mut qualifying_ids = Vec::new();
    for _ in 0..3 {
        let account = create_account(&accounts, &unique_status).await?;
        flag(&pool, account.id).await?;
        qualifying_ids.push(account.id);
    }
    for _ in 0..3 {
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
                    flagged: None,
                    min_flags: Some(1),
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
    let expected_set: HashSet<_> = qualifying_ids.iter().copied().collect();
    assert_eq!(seen_set, expected_set);
    assert_eq!(
        seen.len(),
        qualifying_ids.len(),
        "no id repeated across pages"
    );

    Ok(())
}
