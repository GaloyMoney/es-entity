mod entities;
mod helpers;

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use entities::transfer::*;
use es_entity::*;

/// Repo shape mirroring lana's `Disbursals`: two non-optional filter columns
/// plus one optional filter column, sorted `by(created_at)`, plus a nullable
/// `score` sort column for NULL-cursor edge cases.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Transfer",
    columns(
        account_id(ty = "AccountId", list_for(by(created_at))),
        status(ty = "String", list_for(by(created_at))),
        reference(ty = "Option<String>", list_for),
        score(ty = "Option<i32>", list_by)
    )
)]
pub struct Transfers {
    pool: PgPool,
}

impl Transfers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
struct Row {
    id: uuid::Uuid,
    account_id: uuid::Uuid,
    status: String,
    reference: Option<String>,
    score: Option<i32>,
    created_at: DateTime<Utc>,
}

async fn seed_transfers(
    repo: &Transfers,
    specs: &[(uuid::Uuid, &str, Option<&str>, Option<i32>)],
) -> anyhow::Result<()> {
    for (account_id, status, reference, score) in specs {
        let mut new = NewTransfer::builder()
            .id(TransferId::new())
            .account_id(AccountId::from(*account_id))
            .status(*status)
            .score(*score)
            .build()
            .unwrap();
        new.reference = reference.map(|r| r.to_string());
        repo.create(new).await?;
    }
    Ok(())
}

async fn ground_truth(pool: &PgPool, account_ids: &[uuid::Uuid]) -> anyhow::Result<Vec<Row>> {
    let rows = sqlx::query_as::<
        _,
        (
            uuid::Uuid,
            uuid::Uuid,
            String,
            Option<String>,
            Option<i32>,
            DateTime<Utc>,
        ),
    >(
        "SELECT id, account_id, status, reference, score, created_at FROM transfers WHERE account_id = ANY($1)",
    )
    .bind(account_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, account_id, status, reference, score, created_at)| Row {
                id,
                account_id,
                status,
                reference,
                score,
                created_at,
            },
        )
        .collect())
}

/// Paginating `list_for_filters` through every filter combination x sort x
/// direction must return exactly the same rows in exactly the same order as
/// an in-Rust reference implementation. This is the correctness harness for
/// the specialized query matrix — it exercises the proxy dispatch (dedicated
/// `list_for_*` / `list_by_*` paths), the specialized catch-all variants,
/// and the cursor/no-cursor split on every page transition.
#[tokio::test]
async fn list_for_filters_matches_reference_for_all_combos() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let transfers = Transfers::new(pool.clone());

    let account_ids: Vec<uuid::Uuid> = (0..3).map(|_| uuid::Uuid::from(AccountId::new())).collect();
    let mut specs = Vec::new();
    for i in 0..40 {
        let account_id = account_ids[i % 3];
        let status = if i % 2 == 0 { "active" } else { "settled" };
        let reference = match i % 3 {
            0 => None,
            1 => Some("ref-a"),
            _ => Some("ref-b"),
        };
        specs.push((account_id, status, reference, None));
    }
    seed_transfers(&transfers, &specs).await?;
    let truth = ground_truth(&pool, &account_ids).await?;
    // The table is shared with other tests (and previous runs): scenarios
    // without an account filter legitimately return foreign rows. Ordering
    // is total and deterministic, so filtering the returned stream down to
    // this test's rows preserves everything there is to check.
    let truth_ids: std::collections::HashSet<_> = truth.iter().map(|r| r.id).collect();

    let account_filters: [Option<uuid::Uuid>; 2] = [None, Some(account_ids[0])];
    let status_filters: [Option<&str>; 2] = [None, Some("active")];
    let reference_filters: [Option<Option<&str>>; 3] = [None, Some(None), Some(Some("ref-a"))];
    let sorts = [TransferSortBy::Id, TransferSortBy::CreatedAt];
    let directions = [ListDirection::Ascending, ListDirection::Descending];

    for account_filter in account_filters {
        for status_filter in status_filters {
            for reference_filter in reference_filters {
                for by in sorts {
                    for direction in directions {
                        let expected = reference_order(
                            &truth,
                            account_filter,
                            status_filter,
                            reference_filter,
                            by,
                            direction,
                        );

                        let mut actual = Vec::new();
                        let mut after: Option<transfer_cursor::TransferCursor> = None;
                        loop {
                            let ret = transfers
                                .list_for_filters(
                                    TransferFilters {
                                        account_id: account_filter.map(AccountId::from),
                                        status: status_filter.map(|s| s.to_string()),
                                        reference: reference_filter
                                            .map(|r| r.map(|s| s.to_string())),
                                    },
                                    Sort { by, direction },
                                    PaginatedQueryArgs { first: 7, after },
                                )
                                .await?;
                            actual.extend(ret.entities.iter().map(|t| uuid::Uuid::from(t.id)));
                            if !ret.has_next_page {
                                break;
                            }
                            after = ret.end_cursor;
                            assert!(after.is_some(), "has_next_page without end_cursor");
                        }
                        actual.retain(|id| truth_ids.contains(id));

                        assert_eq!(
                            actual, expected,
                            "mismatch for account={account_filter:?} status={status_filter:?} \
                             reference={reference_filter:?} by={by:?} direction={direction:?}"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn reference_order(
    rows: &[Row],
    account_filter: Option<uuid::Uuid>,
    status_filter: Option<&str>,
    reference_filter: Option<Option<&str>>,
    by: TransferSortBy,
    direction: ListDirection,
) -> Vec<uuid::Uuid> {
    let mut rows: Vec<&Row> = rows
        .iter()
        .filter(|r| account_filter.is_none_or(|a| r.account_id == a))
        .filter(|r| status_filter.is_none_or(|s| r.status == s))
        .filter(|r| match reference_filter {
            None => true,
            Some(None) => r.reference.is_none(),
            Some(Some(v)) => r.reference.as_deref() == Some(v),
        })
        .collect();
    rows.sort_by(|a, b| match by {
        TransferSortBy::Id => a.id.cmp(&b.id),
        _ => (a.created_at, a.id).cmp(&(b.created_at, b.id)),
    });
    if matches!(direction, ListDirection::Descending) {
        rows.reverse();
    }
    rows.into_iter().map(|r| r.id).collect()
}

/// `list_by_score` over a nullable sort column must paginate correctly
/// through NULL and non-NULL cursor values in both directions:
/// ASC sorts NULLs FIRST, DESC sorts NULLs LAST.
#[tokio::test]
async fn list_by_score_paginates_through_nulls() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let transfers = Transfers::new(pool.clone());

    let account_id = uuid::Uuid::from(AccountId::new());
    let scores = [
        None,
        Some(5),
        None,
        Some(3),
        Some(7),
        None,
        Some(1),
        Some(3),
    ];
    let specs: Vec<_> = scores
        .iter()
        .map(|score| (account_id, "score_test", None, *score))
        .collect();
    seed_transfers(&transfers, &specs).await?;
    let truth = ground_truth(&pool, &[account_id]).await?;
    // `list_by_score` is unfiltered over a shared table — retain only this
    // test's rows from the (totally ordered) returned stream.
    let truth_ids: std::collections::HashSet<_> = truth.iter().map(|r| r.id).collect();

    for direction in [ListDirection::Ascending, ListDirection::Descending] {
        // Reference: ASC -> NULLs first (id asc), then values by (score, id)
        // asc. DESC -> values by (score, id) desc, then NULLs (id desc).
        let mut null_rows: Vec<_> = truth.iter().filter(|r| r.score.is_none()).collect();
        let mut value_rows: Vec<_> = truth.iter().filter(|r| r.score.is_some()).collect();
        null_rows.sort_by_key(|r| r.id);
        value_rows.sort_by_key(|r| (r.score, r.id));
        let expected: Vec<uuid::Uuid> = match direction {
            ListDirection::Ascending => null_rows
                .into_iter()
                .chain(value_rows)
                .map(|r| r.id)
                .collect(),
            ListDirection::Descending => value_rows
                .into_iter()
                .rev()
                .chain(null_rows.into_iter().rev())
                .map(|r| r.id)
                .collect(),
        };

        // A page size smaller than the 8 seeded rows forces pagination even
        // on a pristine database: ASC crosses the NULL → value boundary and
        // DESC the value → NULL boundary, so the specialized `After` and
        // `AfterNull` cursor variants both execute in each direction.
        let mut actual = Vec::new();
        let mut after: Option<transfer_cursor::TransferByScoreCursor> = None;
        let mut pages = 0;
        loop {
            let ret = transfers
                .list_by_score(PaginatedQueryArgs { first: 3, after }, direction)
                .await?;
            pages += 1;
            actual.extend(ret.entities.iter().map(|t| uuid::Uuid::from(t.id)));
            if !ret.has_next_page {
                break;
            }
            after = ret.end_cursor;
            assert!(after.is_some(), "has_next_page without end_cursor");
        }
        actual.retain(|id| truth_ids.contains(id));

        assert!(
            pages >= 3,
            "pagination must span multiple pages to exercise the cursor \
             variants, got {pages} page(s) for direction={direction:?}"
        );
        assert_eq!(actual, expected, "mismatch for direction={direction:?}");
    }

    Ok(())
}

/// The dedicated single-filter path (what lana's hot
/// `creditFacility { disbursals }` resolver should dispatch to) filters and
/// paginates correctly on its own.
#[tokio::test]
async fn list_for_account_id_by_created_at_paginates() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let transfers = Transfers::new(pool.clone());

    let account_ids: Vec<uuid::Uuid> = (0..2).map(|_| uuid::Uuid::from(AccountId::new())).collect();
    let specs: Vec<_> = (0..10)
        .map(|i| (account_ids[i % 2], "dedicated", None, None))
        .collect();
    seed_transfers(&transfers, &specs).await?;
    let truth = ground_truth(&pool, &[account_ids[0]]).await?;

    let mut expected: Vec<_> = truth.iter().collect();
    expected.sort_by_key(|r| (r.created_at, r.id));
    let expected: Vec<_> = expected.into_iter().rev().map(|r| r.id).collect();

    let mut actual = Vec::new();
    let mut after: Option<transfer_cursor::TransferByCreatedAtCursor> = None;
    loop {
        let ret = transfers
            .list_for_account_id_by_created_at(
                AccountId::from(account_ids[0]),
                PaginatedQueryArgs { first: 2, after },
                ListDirection::Descending,
            )
            .await?;
        actual.extend(ret.entities.iter().map(|t| uuid::Uuid::from(t.id)));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
    }

    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 5);
    Ok(())
}
