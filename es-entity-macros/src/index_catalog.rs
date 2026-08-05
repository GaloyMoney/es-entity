//! A PostgreSQL migration schema simulator that computes the final set of
//! physical composite indexes per table.
//!
//! This is the `IndexCatalog` source consumed by `#[derive(EsRepo)]` to decide
//! which `list_for_filters` combinations get a specialized sargable query (a
//! combo is specialized only when a matching composite index physically
//! exists). The catalog is derived from the committed migration `.sql` files at
//! macro-expansion time — the migrations directory *is* the source of truth
//! (precedent: `sqlx::migrate!`), so codegen is a deterministic function of the
//! checkout.
//!
//! Each statement is parsed with `sqlparser` (the PostgreSQL dialect) and the
//! relevant DDL — `CREATE [UNIQUE] INDEX`, `CREATE TABLE` inline/table-level
//! `PRIMARY KEY` / `UNIQUE`, `ALTER TABLE ADD/DROP CONSTRAINT`, `DROP INDEX`,
//! `DROP TABLE` — is applied in filename order to a simulated schema. Statements
//! are split first and parsed **individually** (best effort): a statement
//! `sqlparser` cannot parse — an extension, a `CREATE FUNCTION` body, a
//! dialect quirk — is skipped without discarding the rest of the file, and the
//! test-DB verification lint (see `tests/index_catalog_verification.rs`) catches
//! any resulting gap by comparing against `pg_indexes`.

use std::path::Path;

use sqlparser::{
    ast::{
        AlterTableOperation, ColumnOption, Expr, IndexColumn, ObjectName, ObjectType, Statement,
        TableConstraint,
    },
    dialect::PostgreSqlDialect,
    parser::Parser,
};

/// One physical composite index known for a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// Lowercased, unquoted table name (schema qualifier stripped).
    pub table: String,
    /// Ordered index key column names (lowercased/unquoted), with `ASC`/`DESC`,
    /// `NULLS FIRST|LAST` and `INCLUDE (...)` columns stripped. Expression key
    /// elements are recorded as an opaque sentinel that matches no plain column.
    pub columns: Vec<String>,
    /// `true` for `UNIQUE` indexes, `PRIMARY KEY` and `UNIQUE` constraints.
    pub unique: bool,
    /// Index/constraint name if known (lowercased), else `None`.
    pub name: Option<String>,
    /// `true` if the index carries a `WHERE` clause (partial index).
    pub partial: bool,
}

/// The set of indexes that exist after applying every migration statement in
/// filename order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCatalog {
    pub entries: Vec<IndexEntry>,
}

/// Sentinel column recorded for an expression/opaque index key element; it is
/// deliberately un-representable as a real column name so it never matches a
/// plain-column query prefix.
const OPAQUE_COLUMN: &str = "\u{1}expr";

impl IndexCatalog {
    /// Parse every `*.sql` file in `dir` in ascending filename order.
    pub fn from_migrations_dir(dir: &Path) -> Result<Self, String> {
        let mut files: Vec<(String, String)> = Vec::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("could not read migrations dir {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("sql") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let body = std::fs::read_to_string(&path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            files.push((name, body));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self::from_sql_files(&files))
    }

    /// Apply `(filename, sql)` pairs (sorted by filename) to a simulated schema.
    pub fn from_sql_files(files: &[(String, String)]) -> Self {
        let mut sorted: Vec<&(String, String)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));

        let dialect = PostgreSqlDialect {};
        let mut entries: Vec<IndexEntry> = Vec::new();
        for (_, sql) in sorted {
            let stripped = strip_comments(sql);
            for statement in split_statements(&stripped) {
                // Parse each statement individually so one unparseable statement
                // does not discard the rest of the file.
                if let Ok(parsed) = Parser::parse_sql(&dialect, &statement) {
                    for stmt in &parsed {
                        apply_statement(&mut entries, stmt);
                    }
                }
            }
        }
        entries.dedup();
        Self { entries }
    }

    /// Whether a `list_for_filters` query over `table`, filtering on the
    /// (order-insensitive) equality columns `equality_cols` and paginating by
    /// `sort_col`, is backed by a physical composite index: some index's leading
    /// key columns are a permutation of `equality_cols` immediately followed by
    /// `sort_col`. The implied trailing `id` tiebreak and any further index
    /// columns are irrelevant to the prefix match.
    ///
    /// Partial indexes are conservatively ignored (those combos fall back —
    /// correct, just not sargable).
    pub fn specializes(&self, table: &str, equality_cols: &[String], sort_col: &str) -> bool {
        let table = table.to_lowercase();
        let sort = sort_col.to_lowercase();
        let mut eq: Vec<String> = equality_cols.iter().map(|c| c.to_lowercase()).collect();
        eq.sort();

        self.entries.iter().any(|entry| {
            if entry.partial || entry.table != table {
                return false;
            }
            if entry.columns.len() < eq.len() + 1 {
                return false;
            }
            let mut prefix = entry.columns[..eq.len()].to_vec();
            prefix.sort();
            prefix == eq && entry.columns[eq.len()] == sort
        })
    }
}

// ── AST → catalog ───────────────────────────────────────────────────────────

fn apply_statement(entries: &mut Vec<IndexEntry>, stmt: &Statement) {
    match stmt {
        Statement::CreateIndex(create) => {
            let Some(table) = last_ident(&create.table_name) else {
                return;
            };
            push_entry(
                entries,
                IndexEntry {
                    table,
                    columns: create.columns.iter().map(index_column_name).collect(),
                    unique: create.unique,
                    name: create.name.as_ref().and_then(last_ident),
                    partial: create.predicate.is_some(),
                },
            );
        }
        Statement::CreateTable(create) => {
            let Some(table) = last_ident(&create.name) else {
                return;
            };
            // Inline column `PRIMARY KEY` / `UNIQUE` -> single-column unique index.
            for column in &create.columns {
                let inline_unique = column.options.iter().any(|opt| {
                    matches!(
                        opt.option,
                        ColumnOption::PrimaryKey(_) | ColumnOption::Unique(_)
                    )
                });
                if inline_unique {
                    push_entry(
                        entries,
                        IndexEntry {
                            table: table.clone(),
                            columns: vec![column.name.value.to_lowercase()],
                            unique: true,
                            name: None,
                            partial: false,
                        },
                    );
                }
            }
            // Table-level `PRIMARY KEY (...)` / `UNIQUE (...)` constraints.
            for constraint in &create.constraints {
                if let Some(entry) = constraint_entry(&table, constraint) {
                    push_entry(entries, entry);
                }
            }
        }
        Statement::AlterTable(alter) => {
            let Some(table) = last_ident(&alter.name) else {
                return;
            };
            for op in &alter.operations {
                match op {
                    AlterTableOperation::AddConstraint { constraint, .. } => {
                        if let Some(entry) = constraint_entry(&table, constraint) {
                            push_entry(entries, entry);
                        }
                    }
                    AlterTableOperation::DropConstraint { name, .. } => {
                        let name = name.value.to_lowercase();
                        entries.retain(|e| e.name.as_deref() != Some(name.as_str()));
                    }
                    _ => {}
                }
            }
        }
        Statement::Drop {
            object_type, names, ..
        } => match object_type {
            ObjectType::Index => {
                for name in names {
                    if let Some(name) = last_ident(name) {
                        entries.retain(|e| e.name.as_deref() != Some(name.as_str()));
                    }
                }
            }
            ObjectType::Table => {
                for name in names {
                    if let Some(table) = last_ident(name) {
                        entries.retain(|e| e.table != table);
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// A `UNIQUE` / `PRIMARY KEY` table constraint as a unique index entry; other
/// constraint kinds (foreign key, check, …) yield `None`.
fn constraint_entry(table: &str, constraint: &TableConstraint) -> Option<IndexEntry> {
    let (name, columns) = match constraint {
        TableConstraint::PrimaryKey(pk) => (&pk.name, &pk.columns),
        TableConstraint::Unique(u) => (&u.name, &u.columns),
        _ => return None,
    };
    Some(IndexEntry {
        table: table.to_string(),
        columns: columns.iter().map(index_column_name).collect(),
        unique: true,
        name: name.as_ref().map(|i| i.value.to_lowercase()),
        partial: false,
    })
}

/// The last identifier of a (possibly schema-qualified) object name, lowercased.
fn last_ident(name: &ObjectName) -> Option<String> {
    name.0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.to_lowercase())
}

/// The plain column name of an index key element (`ASC`/`DESC`/`NULLS` live in
/// the ordering options, not the expression), or the opaque sentinel for an
/// expression key.
fn index_column_name(column: &IndexColumn) -> String {
    match &column.column.expr {
        Expr::Identifier(ident) => ident.value.to_lowercase(),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .map(|ident| ident.value.to_lowercase())
            .unwrap_or_else(|| OPAQUE_COLUMN.to_string()),
        _ => OPAQUE_COLUMN.to_string(),
    }
}

fn push_entry(entries: &mut Vec<IndexEntry>, entry: IndexEntry) {
    if !entries.contains(&entry) {
        entries.push(entry);
    }
}

// ── statement splitting ─────────────────────────────────────────────────────

/// Strip line (`-- …`) and block (`/* … */`) comments, preserving string
/// literals verbatim, so a `;` inside a comment cannot split a statement.
fn strip_comments(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\'' {
                    i += 1;
                    if i < chars.len() && chars[i] == '\'' {
                        out.push('\'');
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Split on top-level `;`, ignoring semicolons inside `'…'` string literals and
/// `$tag$…$tag$` dollar-quoted bodies (e.g. `CREATE FUNCTION`).
fn split_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            cur.push(c);
            i += 1;
            while i < chars.len() {
                cur.push(chars[i]);
                if chars[i] == '\'' {
                    i += 1;
                    if i < chars.len() && chars[i] == '\'' {
                        cur.push('\'');
                        i += 1;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            continue;
        }
        if c == '$'
            && let Some(tag) = dollar_tag(&chars, i)
        {
            // Copy the whole dollar-quoted body verbatim, tags included.
            let end = find_dollar_close(&chars, i + tag.len(), &tag);
            for &ch in &chars[i..end] {
                cur.push(ch);
            }
            i = end;
            continue;
        }
        if c == ';' {
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// If a `$tag$` (or `$$`) dollar-quote tag opens at `start`, return the tag
/// including both `$` delimiters.
fn dollar_tag(chars: &[char], start: usize) -> Option<String> {
    debug_assert_eq!(chars[start], '$');
    let mut j = start + 1;
    while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
        j += 1;
    }
    if j < chars.len() && chars[j] == '$' {
        Some(chars[start..=j].iter().collect())
    } else {
        None
    }
}

/// Index just past the closing dollar-quote `tag`, searching from `from`.
fn find_dollar_close(chars: &[char], from: usize, tag: &str) -> usize {
    let tag: Vec<char> = tag.chars().collect();
    let mut i = from;
    while i + tag.len() <= chars.len() {
        if chars[i..i + tag.len()] == tag[..] {
            return i + tag.len();
        }
        i += 1;
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(sql: &str) -> IndexCatalog {
        IndexCatalog::from_sql_files(&[("001_test.sql".to_string(), sql.to_string())])
    }

    #[test]
    fn plain_index() {
        let c = cat("CREATE INDEX ON t (a, b);");
        assert_eq!(c.entries.len(), 1);
        let e = &c.entries[0];
        assert_eq!(e.table, "t");
        assert_eq!(e.columns, vec!["a", "b"]);
        assert!(!e.unique);
        assert!(!e.partial);
        assert_eq!(e.name, None);
    }

    #[test]
    fn unique_named_index() {
        let c = cat("CREATE UNIQUE INDEX idx_t_a ON t (a);");
        let e = &c.entries[0];
        assert!(e.unique);
        assert_eq!(e.name.as_deref(), Some("idx_t_a"));
        assert_eq!(e.columns, vec!["a"]);
    }

    #[test]
    fn strips_direction_and_nulls_ordering() {
        let c = cat("CREATE INDEX ON t (created_at DESC NULLS LAST, id ASC);");
        assert_eq!(c.entries[0].columns, vec!["created_at", "id"]);
    }

    #[test]
    fn partial_index_flagged() {
        let c = cat("CREATE INDEX ON t (a) WHERE deleted = FALSE;");
        assert!(c.entries[0].partial);
        assert_eq!(c.entries[0].columns, vec!["a"]);
    }

    #[test]
    fn include_columns_excluded() {
        let c = cat("CREATE INDEX ON t (a) INCLUDE (b, c);");
        assert_eq!(c.entries[0].columns, vec!["a"]);
    }

    #[test]
    fn schema_qualifier_and_quoted_idents_lowercased() {
        let c = cat("CREATE INDEX ON public.\"My_Tbl\" (\"Col_A\", col_b);");
        let e = &c.entries[0];
        assert_eq!(e.table, "my_tbl");
        assert_eq!(e.columns, vec!["col_a", "col_b"]);
    }

    #[test]
    fn create_table_inline_primary_key() {
        let c = cat("CREATE TABLE t (id uuid PRIMARY KEY, a int);");
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].columns, vec!["id"]);
        assert!(c.entries[0].unique);
    }

    #[test]
    fn create_table_table_level_constraints_and_nested_commas() {
        let c = cat(
            "CREATE TABLE t (a int, b int, price numeric(10,2), PRIMARY KEY (a, b), UNIQUE (b));",
        );
        assert!(
            c.entries
                .iter()
                .any(|e| e.columns == vec!["a", "b"] && e.unique)
        );
        assert!(c.entries.iter().any(|e| e.columns == vec!["b"] && e.unique));
        // `numeric(10,2)` must not have split the item list on its inner comma.
        assert_eq!(c.entries.len(), 2);
    }

    #[test]
    fn foreign_key_column_not_treated_as_unique() {
        let c = cat("CREATE TABLE t (id uuid PRIMARY KEY, other_id uuid REFERENCES others(id));");
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].columns, vec!["id"]);
    }

    #[test]
    fn alter_add_then_drop_constraint() {
        let c = cat("CREATE TABLE t (a int, b int); \
             ALTER TABLE t ADD CONSTRAINT t_ab_key UNIQUE (a, b); \
             ALTER TABLE t DROP CONSTRAINT t_ab_key;");
        assert!(c.entries.is_empty());
    }

    #[test]
    fn drop_index_removes_by_name() {
        let c = cat("CREATE INDEX my_idx ON t (a); DROP INDEX my_idx;");
        assert!(c.entries.is_empty());
    }

    #[test]
    fn drop_table_removes_all_its_indexes() {
        let c = cat("CREATE INDEX ON t (a); CREATE INDEX ON u (a); DROP TABLE t;");
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].table, "u");
    }

    #[test]
    fn filename_order_applies_later_drop() {
        let files = [
            ("002_drop.sql".to_string(), "DROP INDEX my_idx;".to_string()),
            (
                "001_create.sql".to_string(),
                "CREATE INDEX my_idx ON t (a);".to_string(),
            ),
        ];
        let c = IndexCatalog::from_sql_files(&files);
        assert!(c.entries.is_empty());
    }

    #[test]
    fn comments_stripped_including_semicolon_in_comment() {
        let c = cat("-- a comment with ; inside\n\
             CREATE INDEX ON t (a); /* block ; comment */ CREATE INDEX ON u (b);");
        assert_eq!(c.entries.len(), 2);
    }

    #[test]
    fn dollar_quoted_body_does_not_split_or_leak_indexes() {
        // A function body containing `;` and even `CREATE INDEX`-looking text
        // must not be mis-split into spurious statements.
        let c = cat(
            "CREATE FUNCTION f() RETURNS void AS $$ BEGIN CREATE INDEX ON t (a); END; $$ LANGUAGE plpgsql; \
             CREATE INDEX real_idx ON u (b);",
        );
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].table, "u");
        assert_eq!(c.entries[0].name.as_deref(), Some("real_idx"));
    }

    #[test]
    fn specializes_prefix_permutation_then_sort() {
        // (account_id, created_at, id) supports {account_id} + sort created_at.
        let c = cat("CREATE INDEX ON transfers (account_id, created_at DESC, id);");
        assert!(c.specializes("transfers", &["account_id".to_string()], "created_at"));
        // Equality set is order-insensitive within the prefix.
        let c2 = cat("CREATE INDEX ON t (a, b, created_at, id);");
        assert!(c2.specializes("t", &["b".to_string(), "a".to_string()], "created_at"));
        // Sort must immediately follow the equality prefix.
        assert!(!c.specializes("transfers", &["account_id".to_string()], "id"));
        // Missing equality column → no match.
        assert!(!c.specializes("transfers", &[], "created_at"));
        // Partial indexes are ignored.
        let c3 = cat("CREATE INDEX ON t (a, created_at) WHERE deleted = FALSE;");
        assert!(!c3.specializes("t", &["a".to_string()], "created_at"));
    }
}
