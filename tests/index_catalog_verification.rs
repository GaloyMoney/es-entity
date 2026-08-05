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

/// Mirrors `IndexCatalog::specializes`: some index's leading key columns are a
/// permutation of `equality_cols` immediately followed by `sort_col`.
fn covered(indexes: &[Vec<String>], equality_cols: &[&str], sort_col: &str) -> bool {
    let mut eq: Vec<String> = equality_cols.iter().map(|c| c.to_lowercase()).collect();
    eq.sort();
    indexes.iter().any(|cols| {
        if cols.len() < eq.len() + 1 {
            return false;
        }
        let mut prefix = cols[..eq.len()].to_vec();
        prefix.sort();
        prefix == eq && cols[eq.len()] == sort_col.to_lowercase()
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

    // A combination with no matching index must correctly report *not* covered
    // (guards the check itself against always-true bugs): transfers has no
    // (status, created_at) index.
    assert!(
        !covered(&transfers, &["status"], "created_at"),
        "sanity: transfers has no (status, created_at) index"
    );

    conn.close().await?;
    Ok(())
}
