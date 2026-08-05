//! Acceptance tests for the unified cursor query emission.
//!
//! Since this change (unified `UNION ALL` cursor queries replacing the
//! per-cursor-state matrix) two guarantees must hold and are covered here:
//!
//!  1. **Semantics are byte-identical** to the previous per-state / catch-all
//!     forms — pagination through NULL and non-NULL sort values, in both
//!     directions, must produce the exact same rows in the exact same order.
//!     `unified_list_by_nullable_matches_reference_at_every_cursor` paginates
//!     with page size 1 (and 2), so *every* row acts as a cursor and every
//!     `NULL ↔ value` boundary is crossed, comparing against a Rust reference
//!     ordering.
//!  2. **The single query stays sargable** — Postgres lifts the per-branch
//!     nullness guards into `One-Time Filter` nodes, skips the dead branches at
//!     execution (`never executed`), and the live branch keeps the composite
//!     `(col, id)` index qual. `unified_cursor_query_prunes_dead_branches`
//!     asserts this on the pinned Postgres via `EXPLAIN ANALYZE`, guarding
//!     against planner-behaviour drift across PG upgrades.

mod entities;
mod helpers;

use sqlx::{PgPool, Row};

use entities::transfer::*;
use es_entity::*;

/// Minimal repo over the shared `transfers` table with a nullable `score`
/// sort column driving `list_by`.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Transfer",
    columns(
        account_id(ty = "AccountId"),
        status(ty = "String"),
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

async fn seed(
    repo: &Transfers,
    account_id: uuid::Uuid,
    scores: &[Option<i32>],
) -> anyhow::Result<()> {
    for score in scores {
        let new = NewTransfer::builder()
            .id(TransferId::new())
            .account_id(AccountId::from(account_id))
            .status("unified_cursor_test")
            .score(*score)
            .build()
            .unwrap();
        repo.create(new).await?;
    }
    Ok(())
}

/// The Rust ground-truth order for `list_by_score`: ASC sorts NULLs first
/// (then values by `(score, id)`), DESC sorts values by `(score, id)` desc then
/// NULLs — mirroring `score ASC/DESC NULLS FIRST/LAST, id ASC/DESC`.
fn reference_order(
    rows: &[(uuid::Uuid, Option<i32>)],
    direction: ListDirection,
) -> Vec<uuid::Uuid> {
    let mut nulls: Vec<_> = rows.iter().filter(|(_, s)| s.is_none()).collect();
    let mut values: Vec<_> = rows.iter().filter(|(_, s)| s.is_some()).collect();
    nulls.sort_by_key(|(id, _)| *id);
    values.sort_by_key(|(id, s)| (*s, *id));
    match direction {
        ListDirection::Ascending => nulls.into_iter().chain(values).map(|(id, _)| *id).collect(),
        ListDirection::Descending => values
            .into_iter()
            .rev()
            .chain(nulls.into_iter().rev())
            .map(|(id, _)| *id)
            .collect(),
    }
}

/// Paginate `list_by_score` fully at the given page size, retaining only the
/// ids we seeded (the table is shared with other tests, but the ordering is
/// total so filtering the returned stream preserves everything under test).
async fn paginate(
    repo: &Transfers,
    direction: ListDirection,
    first: usize,
    ours: &std::collections::HashSet<uuid::Uuid>,
) -> anyhow::Result<Vec<uuid::Uuid>> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let ret = repo
            .list_by_score(PaginatedQueryArgs { first, after }, direction)
            .await?;
        out.extend(ret.entities.iter().map(|t| uuid::Uuid::from(t.id)));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
        assert!(after.is_some(), "has_next_page without end_cursor");
    }
    out.retain(|id| ours.contains(id));
    Ok(out)
}

/// A page size of 1 makes *every* row act as a cursor, so both the `After`
/// (cursor on a non-NULL score) and `AfterNull` (cursor on a NULL score)
/// branches of the unified query — plus every `NULL ↔ value` boundary — are
/// exercised in both directions. Page size 2 adds a second stride offset.
#[tokio::test]
async fn unified_list_by_nullable_matches_reference_at_every_cursor() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Transfers::new(pool.clone());

    let account_id = uuid::Uuid::from(AccountId::new());
    // Interleave NULLs and duplicate values so the (score, id) tiebreak and the
    // NULL boundaries are both under test.
    let scores = [
        None,
        Some(5),
        None,
        Some(3),
        Some(7),
        None,
        Some(3),
        Some(1),
        None,
        Some(5),
    ];
    seed(&repo, account_id, &scores).await?;

    let rows: Vec<(uuid::Uuid, Option<i32>)> = {
        let mut r = Vec::new();
        for row in sqlx::query("SELECT id, score FROM transfers WHERE account_id = $1")
            .bind(account_id)
            .fetch_all(&pool)
            .await?
        {
            r.push((
                row.get::<uuid::Uuid, _>("id"),
                row.get::<Option<i32>, _>("score"),
            ));
        }
        r
    };
    let ours: std::collections::HashSet<uuid::Uuid> = rows.iter().map(|(id, _)| *id).collect();

    for direction in [ListDirection::Ascending, ListDirection::Descending] {
        let expected = reference_order(&rows, direction);
        for first in [1usize, 2] {
            let actual = paginate(&repo, direction, first, &ours).await?;
            assert_eq!(
                actual, expected,
                "unified cursor pagination diverged from reference \
                 (direction={direction:?}, page_size={first})"
            );
        }
    }

    Ok(())
}

/// The unified query is one static `UNION ALL`; Postgres must lift the
/// parameter-only nullness guards into `One-Time Filter` nodes so the dead
/// branches are skipped (`never executed`) and the live `After` branch keeps
/// the sargable composite-index qual. Verified on a single connection with a
/// generic plan (the pooled prepared-statement condition) so the assertion
/// reflects production planning, not a one-off custom plan.
#[tokio::test]
async fn unified_cursor_query_prunes_dead_branches() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let mut tx = pool.begin().await?;

    sqlx::query("SET LOCAL plan_cache_mode = force_generic_plan")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE TEMP TABLE u (id uuid, score int) ON COMMIT DROP")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO u SELECT gen_random_uuid(), \
         CASE WHEN g % 5 = 0 THEN NULL ELSE g END FROM generate_series(1, 300) g",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("CREATE INDEX ON u (score, id)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("CREATE INDEX ON u (id)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("ANALYZE u").execute(&mut *tx).await?;

    // A cursor sitting mid-table on a non-NULL score → the `After` branch is
    // live, `AfterNull` and `First` must be pruned.
    let mid_id: uuid::Uuid = sqlx::query("SELECT id FROM u WHERE score = 151 LIMIT 1")
        .fetch_one(&mut *tx)
        .await?
        .get(0);

    // The exact unified ASC query shape emitted by `assemble_union_select` for
    // an `Option<i32>` sort column: predicate-before-gate, page-1 branch last,
    // per-branch ORDER BY + LIMIT, outer ORDER BY + LIMIT. `$1` = limit,
    // `$2` = cursor id, `$3` = cursor score. Prepared with explicit parameter
    // types and executed via `EXECUTE` so `force_generic_plan` yields the
    // pooled prepared-statement plan production uses (the guards stay symbolic,
    // so the plan shows `never executed` rather than a constant-folded custom
    // plan). `raw_sql` is used so sqlx does not treat the `$n` placeholders as
    // its own bind parameters.
    sqlx::raw_sql(
        "PREPARE uq(int8, uuid, int) AS \
         (SELECT score, id FROM u WHERE ((score, id) > ($3, $2)) AND ($2 IS NOT NULL AND $3 IS NOT NULL) ORDER BY score ASC NULLS FIRST, id ASC LIMIT $1) \
         UNION ALL (SELECT score, id FROM u WHERE ((score IS NOT NULL OR id > $2)) AND ($2 IS NOT NULL AND $3 IS NULL) ORDER BY score ASC NULLS FIRST, id ASC LIMIT $1) \
         UNION ALL (SELECT score, id FROM u WHERE ($2 IS NULL) ORDER BY score ASC NULLS FIRST, id ASC LIMIT $1) \
         ORDER BY score ASC NULLS FIRST, id ASC LIMIT $1",
    )
    .execute(&mut *tx)
    .await?;

    let plan = sqlx::raw_sql(&format!(
        "EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) EXECUTE uq(5, '{mid_id}'::uuid, 151)"
    ))
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| r.get::<String, _>(0))
    .collect::<Vec<_>>()
    .join("\n");

    tx.rollback().await?;

    // Guards lifted to One-Time Filters (a prerequisite for branch pruning).
    assert!(
        plan.contains("One-Time Filter"),
        "expected the nullness guards to be lifted into One-Time Filter nodes, got:\n{plan}"
    );
    // Dead branches are skipped at execution.
    assert!(
        plan.contains("never executed"),
        "expected the non-live branches to be pruned (never executed), got:\n{plan}"
    );
    // The live `After` branch keeps the sargable composite-index row qual.
    assert!(
        plan.contains("Index Cond: (ROW(score, id) > ROW($3, $2))"),
        "expected the live branch to keep the sargable (score, id) index qual, got:\n{plan}"
    );

    Ok(())
}
