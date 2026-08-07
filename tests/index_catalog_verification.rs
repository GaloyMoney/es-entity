//! Verification lint for the index catalog: the physical composite indexes the
//! `list_for_filters` specialization relies on must actually exist in the
//! migrated database.
//!
//! `#[derive(EsRepo)]` decides which filter combinations get a specialized
//! sargable query by parsing the committed migration `.sql` files at
//! macro-expansion time. If a migration is edited/renamed such that a relied-on
//! index no longer exists, specialization would silently de-specialize (a build
//! regression with no runtime error) — this test turns that into a loud
//! failure by checking the live `pg_indexes` on a single acquired connection
//! (the #162 flake lesson: never probe plans/catalogs through the pool).

use sqlx::{Connection, PgConnection, Row};

/// The ordered key columns of every btree index on `table`, `DESC`/`NULLS ...`
/// stripped, `INCLUDE`/`WHERE` clauses ignored.
async fn index_key_columns(
    conn: &mut PgConnection,
    table: &str,
) -> anyhow::Result<Vec<Vec<String>>> {
    let rows = sqlx::query("SELECT indexdef FROM pg_indexes WHERE tablename = $1")
        .bind(table)
        .fetch_all(&mut *conn)
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let def: String = row.get("indexdef");
        // Key list is the first parenthesised group after `USING <method> `.
        let Some((_, rest)) = def.split_once("btree (") else {
            continue;
        };
        let Some((cols, _)) = rest.split_once(')') else {
            continue;
        };
        let columns = cols
            .split(',')
            .map(|c| {
                c.trim()
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_lowercase()
            })
            .collect();
        out.push(columns);
    }
    Ok(out)
}

/// Mirrors `IndexCatalog::specializes`: the equality columns are a **leading
/// prefix** of some index — any key columns may follow, the sort column need
/// not immediately succeed the prefix. A `col = $k` predicate is an index qual
/// on such an index regardless of what trails it, which the `COALESCE` fallback
/// can never be; that alone makes the specialization preferable (#185). With no
/// equality filter the only benefit is index-ordered pagination, so the sort
/// column must itself lead an index.
fn covered(indexes: &[Vec<String>], equality_cols: &[&str], sort_col: &str) -> bool {
    let sort = sort_col.to_lowercase();
    let mut eq: Vec<String> = equality_cols.iter().map(|c| c.to_lowercase()).collect();
    eq.sort();
    indexes.iter().any(|cols| {
        if cols.len() < eq.len() {
            return false;
        }
        let mut prefix = cols[..eq.len()].to_vec();
        prefix.sort();
        if prefix != eq {
            return false;
        }
        if eq.is_empty() {
            cols.first().map(String::as_str) == Some(sort.as_str())
        } else {
            true
        }
    })
}

#[tokio::test]
async fn specialization_relied_on_indexes_physically_exist() -> anyhow::Result<()> {
    // Single acquired connection — do not probe the catalog through the pool.
    let conn_str = std::env::var("PG_CON")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .expect("PG_CON or DATABASE_URL must be set");
    let mut conn = PgConnection::connect(&conn_str).await?;

    // Every (table, equality-prefix, sort) that a test repo's specialized
    // `list_for_filters` combo depends on must be backed by a real index.
    // `tests/sargable_list_queries.rs` `Transfers`: filter account_id, sort
    // created_at.
    let transfers = index_key_columns(&mut conn, "transfers").await?;
    assert!(
        covered(&transfers, &["account_id"], "created_at"),
        "transfers must have an index covering (account_id, created_at, ...); \
         got {transfers:?}"
    );

    // `tests/scoped_repo_sargable.rs` `Contacts` (scope partner_id, sort
    // created_at): the scoped Only-arm auto-prefixes the scope column.
    let contacts = index_key_columns(&mut conn, "contacts").await?;
    assert!(
        covered(&contacts, &["partner_id"], "created_at"),
        "contacts must have an index covering (partner_id, created_at, ...); \
         got {contacts:?}"
    );

    // Newly-permitted shape (#185): a bare `(status)` index covers the status
    // listing sorted by created_at even though the sort column does not follow
    // it in the index — the exact combo the pre-fix mirror wrongly reported
    // uncovered while `specializes` accepted it.
    assert!(
        covered(&transfers, &["status"], "created_at"),
        "transfers has a bare (status) index; the relaxed mirror must cover it; \
         got {transfers:?}"
    );

    // A combination with no matching index must correctly report *not* covered
    // (guards the check itself against always-true bugs): `reference` is
    // unindexed, so no index has it as a leading prefix.
    assert!(
        !covered(&transfers, &["reference"], "created_at"),
        "sanity: transfers has no index on `reference`"
    );

    conn.close().await?;
    Ok(())
}

/// Pins the `covered` mirror to `IndexCatalog::specializes` (macros crate
/// `specializes_on_equality_prefix`) on the shapes #185 newly permitted — an
/// equality prefix whose index does *not* continue with the sort column. Pure
/// (no DB), so it also runs under offline `nix flake check`. The two functions
/// live in different crates (the proc-macro crate cannot export the catalog),
/// so this hand-checked parallel is the only thing keeping the mirror honest.
#[test]
fn covered_mirrors_specializes() {
    // (account_id, created_at, id) supports {account_id} + sort created_at.
    let transfers = vec![vec![
        "account_id".to_string(),
        "created_at".to_string(),
        "id".to_string(),
    ]];
    assert!(covered(&transfers, &["account_id"], "created_at"));
    // Equality set is order-insensitive within the prefix.
    let abc = vec![vec![
        "a".to_string(),
        "b".to_string(),
        "created_at".to_string(),
        "id".to_string(),
    ]];
    assert!(covered(&abc, &["b", "a"], "created_at"));
    // NEWLY PERMITTED (#185): the equality being a leading prefix is sufficient —
    // the sort column need not immediately follow it.
    assert!(covered(&transfers, &["account_id"], "id"));
    // NEWLY PERMITTED (#185): a bare `(status)` index covers the status listing
    // sorted by any column — the combo the pre-fix mirror wrongly rejected.
    let bare = vec![vec!["status".to_string()]];
    assert!(covered(&bare, &["status"], "created_at"));
    // No index with the equality column as a leading prefix → not covered.
    assert!(!covered(&bare, &["obligation_id"], "created_at"));
    // No equality filter: the sort column must lead an index (preserved).
    assert!(!covered(&transfers, &[], "created_at"));
    assert!(covered(&transfers, &[], "account_id"));
}
