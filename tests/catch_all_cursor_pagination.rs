mod entities;
mod helpers;

use sqlx::PgPool;

use entities::transfer::*;
use es_entity::*;

/// Same entity/table as the sargable list tests, but the nullable `score`
/// sort column opts out of the per-cursor-state SQL matrix via
/// `cursor = "catch_all"`: a single catch-all COALESCE query per direction.
/// Result semantics must be identical to the sargable per-state queries.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Transfer",
    columns(
        account_id(ty = "AccountId", list_for(by(score))),
        status(ty = "String"),
        reference(ty = "Option<String>"),
        score(ty = "Option<i32>", list_by, cursor = "catch_all")
    )
)]
pub struct TransfersCatchAll {
    pool: PgPool,
}

impl TransfersCatchAll {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed(
    repo: &TransfersCatchAll,
    account_id: uuid::Uuid,
    scores: &[Option<i32>],
) -> anyhow::Result<()> {
    for score in scores {
        let new = NewTransfer::builder()
            .id(TransferId::new())
            .account_id(AccountId::from(account_id))
            .status("catch_all_test")
            .score(*score)
            .build()
            .unwrap();
        repo.create(new).await?;
    }
    Ok(())
}

async fn rows_for_account(
    pool: &PgPool,
    account_id: uuid::Uuid,
) -> anyhow::Result<Vec<(uuid::Uuid, Option<i32>)>> {
    let rows = sqlx::query_as::<_, (uuid::Uuid, Option<i32>)>(
        "SELECT id, score FROM transfers WHERE account_id = $1",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// ASC: NULLs first (id asc), then values by (score, id) asc.
/// DESC: values by (score, id) desc, then NULLs (id desc).
fn reference_order(
    rows: &[(uuid::Uuid, Option<i32>)],
    direction: ListDirection,
) -> Vec<uuid::Uuid> {
    let mut null_rows: Vec<_> = rows.iter().filter(|r| r.1.is_none()).collect();
    let mut value_rows: Vec<_> = rows.iter().filter(|r| r.1.is_some()).collect();
    null_rows.sort_by_key(|r| r.0);
    value_rows.sort_by_key(|r| (r.1, r.0));
    match direction {
        ListDirection::Ascending => null_rows
            .into_iter()
            .chain(value_rows)
            .map(|r| r.0)
            .collect(),
        ListDirection::Descending => value_rows
            .into_iter()
            .rev()
            .chain(null_rows.into_iter().rev())
            .map(|r| r.0)
            .collect(),
    }
}

/// `list_by_score` with `cursor = "catch_all"` must paginate through NULL
/// and non-NULL cursor values in both directions with the exact same
/// semantics as the sargable per-state queries.
#[tokio::test]
async fn list_by_score_catch_all_paginates_through_nulls() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let transfers = TransfersCatchAll::new(pool.clone());

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
    seed(&transfers, account_id, &scores).await?;
    let truth = rows_for_account(&pool, account_id).await?;
    // `list_by_score` is unfiltered over a shared table — retain only this
    // test's rows from the (totally ordered) returned stream.
    let truth_ids: std::collections::HashSet<_> = truth.iter().map(|r| r.0).collect();

    for direction in [ListDirection::Ascending, ListDirection::Descending] {
        let expected = reference_order(&truth, direction);

        // A page size smaller than the 8 seeded rows forces pagination even
        // on a pristine database: ASC crosses the NULL → value boundary and
        // DESC the value → NULL boundary, so the catch-all query paginates
        // from both NULL and non-NULL cursor values in each direction.
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
             transitions, got {pages} page(s) for direction={direction:?}"
        );
        assert_eq!(actual, expected, "mismatch for direction={direction:?}");
    }

    Ok(())
}

/// `list_for_account_id_by_score` must honor the by-column's catch-all mode
/// with the same semantics: filter + paginate through NULLs, both
/// directions.
#[tokio::test]
async fn list_for_account_id_by_score_catch_all_paginates() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let transfers = TransfersCatchAll::new(pool.clone());

    let account_id = uuid::Uuid::from(AccountId::new());
    let scores = [None, Some(5), None, Some(3), Some(7), None, Some(1)];
    seed(&transfers, account_id, &scores).await?;
    let truth = rows_for_account(&pool, account_id).await?;

    for direction in [ListDirection::Ascending, ListDirection::Descending] {
        let expected = reference_order(&truth, direction);

        let mut actual = Vec::new();
        let mut after: Option<transfer_cursor::TransferByScoreCursor> = None;
        let mut pages = 0;
        loop {
            let ret = transfers
                .list_for_account_id_by_score(
                    AccountId::from(account_id),
                    PaginatedQueryArgs { first: 2, after },
                    direction,
                )
                .await?;
            pages += 1;
            actual.extend(ret.entities.iter().map(|t| uuid::Uuid::from(t.id)));
            if !ret.has_next_page {
                break;
            }
            after = ret.end_cursor;
            assert!(after.is_some(), "has_next_page without end_cursor");
        }

        assert!(
            pages >= 3,
            "pagination must span multiple pages, got {pages} page(s) for \
             direction={direction:?}"
        );
        assert_eq!(actual, expected, "mismatch for direction={direction:?}");
    }

    Ok(())
}
