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

/// The kind of a classified table constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    /// `UNIQUE` / `PRIMARY KEY` constraints and `UNIQUE` indexes.
    Unique,
    /// `FOREIGN KEY` / `REFERENCES` constraints.
    ForeignKey,
    /// `CHECK` constraints.
    Check,
}

/// One named (or deterministically Postgres-auto-named) constraint on a table
/// whose violation the generated repo error classifier can type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintEntry {
    /// Lowercased, unquoted table name (schema qualifier stripped).
    pub table: String,
    /// Constraint (or unique index) name as reported by Postgres at violation
    /// time — explicit when the DDL names it, else synthesized with Postgres'
    /// auto-naming convention.
    pub name: String,
    pub kind: ConstraintKind,
}

/// The set of indexes that exist after applying every migration statement in
/// filename order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCatalog {
    pub entries: Vec<IndexEntry>,
    /// Classified constraints (unique / foreign key / check) per table.
    pub constraints: Vec<ConstraintEntry>,
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
        let mut constraints: Vec<ConstraintEntry> = Vec::new();
        for (_, sql) in sorted {
            let stripped = strip_comments(sql);
            for statement in split_statements(&stripped) {
                // Parse each statement individually so one unparsable statement
                // does not discard the rest of the file.
                if let Ok(parsed) = Parser::parse_sql(&dialect, &statement) {
                    for stmt in &parsed {
                        apply_statement(&mut entries, &mut constraints, stmt);
                    }
                }
            }
        }
        entries.dedup();
        constraints.dedup();
        Self {
            entries,
            constraints,
        }
    }

    /// Whether a `list_for_filters` query over `table`, filtering on the
    /// (order-insensitive) equality columns `equality_cols` and paginating by
    /// `sort_col`, should emit the sargable specialization rather than the
    /// `COALESCE` fallback.
    ///
    /// It should whenever the equality columns are a **leading prefix** of some
    /// (non-partial) index — then the specialized query's `col = $k` predicates
    /// are an index qual (an index scan bounded to the matching rows), which the
    /// `COALESCE(col = $k, $k IS NULL)` fallback can never be (the `COALESCE`
    /// defeats index extraction, forcing a full sequential scan). That alone
    /// makes the specialization strictly preferable; the sort column following
    /// the equality prefix is a *further* optimisation (the planner can skip the
    /// sort node), not a requirement.
    ///
    /// Requiring the full `(equality…, sort)` composite — the previous behaviour
    /// — silently sent single-filter listings that were backed by only a
    /// `(filter)` index (no `(filter, sort)` composite) to the seq-scanning
    /// fallback, a regression whose cost grew linearly with table size. When
    /// the equality columns match *no* index the two plans both seq-scan, so
    /// the fallback is correct and saves a query.
    ///
    /// With no equality filter the only benefit is index-ordered pagination, so
    /// the sort column must lead an index. Partial indexes are conservatively
    /// ignored (those combos fall back — correct, just not sargable).
    pub fn specializes(&self, table: &str, equality_cols: &[String], sort_col: &str) -> bool {
        let table = table.to_lowercase();
        let sort = sort_col.to_lowercase();
        let mut eq: Vec<String> = equality_cols.iter().map(|c| c.to_lowercase()).collect();
        eq.sort();

        self.entries.iter().any(|entry| {
            if entry.partial || entry.table != table {
                return false;
            }
            if entry.columns.len() < eq.len() {
                return false;
            }
            let mut prefix = entry.columns[..eq.len()].to_vec();
            prefix.sort();
            if prefix != eq {
                return false;
            }
            if eq.is_empty() {
                entry.columns.first().map(String::as_str) == Some(sort.as_str())
            } else {
                true
            }
        })
    }

    /// The names of every *named* unique index/constraint on `table` whose
    /// **last** key column is `column`. Used to map a duplicate-key database
    /// error back to the offending column for a typed error.
    ///
    /// A composite `UNIQUE (a, b)` violation is attributed to its last column
    /// (`b`) — the discriminating column, with the leading columns acting as
    /// the scope (this is the convention downstream repos already follow when
    /// hand-declaring these). Unnamed inline `UNIQUE` / `PRIMARY KEY`
    /// constraints are excluded because their runtime name is Postgres-generated
    /// (covered by the `{table}_{col}_key` / `{table}_pkey` convention the
    /// caller adds instead).
    pub fn unique_index_names(&self, table: &str, column: &str) -> Vec<String> {
        let table = table.to_lowercase();
        let column = column.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                entry.unique && entry.table == table && entry.columns.last() == Some(&column)
            })
            .filter_map(|entry| entry.name.clone())
            .collect()
    }

    /// Every classified constraint on `table` as `(name, kind)` pairs, deduped
    /// by name in first-seen order. Feeds the generated `{Entity}Constraint`
    /// enum.
    pub fn table_constraints(&self, table: &str) -> Vec<(String, ConstraintKind)> {
        let table = table.to_lowercase();
        let mut seen: Vec<(String, ConstraintKind)> = Vec::new();
        for entry in self.constraints.iter().filter(|c| c.table == table) {
            if !seen.iter().any(|(name, _)| name == &entry.name) {
                seen.push((entry.name.clone(), entry.kind));
            }
        }
        seen
    }
}

// ── AST → catalog ───────────────────────────────────────────────────────────

fn apply_statement(
    entries: &mut Vec<IndexEntry>,
    constraints: &mut Vec<ConstraintEntry>,
    stmt: &Statement,
) {
    match stmt {
        Statement::CreateIndex(create) => {
            let Some(table) = last_ident(&create.table_name) else {
                return;
            };
            let columns: Vec<String> = create.columns.iter().map(index_column_name).collect();
            let name = create.name.as_ref().and_then(last_ident);
            if create.unique {
                // Postgres names an unnamed index `{table}_{cols}_idx`.
                let constraint_name = name
                    .clone()
                    .or_else(|| synthesized_name(&table, &columns, "idx"));
                if let Some(constraint_name) = constraint_name {
                    push_constraint(constraints, &table, constraint_name, ConstraintKind::Unique);
                }
            }
            push_entry(
                entries,
                IndexEntry {
                    table,
                    columns,
                    unique: create.unique,
                    name,
                    partial: create.predicate.is_some(),
                },
            );
        }
        Statement::CreateTable(create) => {
            let Some(table) = last_ident(&create.name) else {
                return;
            };
            // Inline column `PRIMARY KEY` / `UNIQUE` -> single-column unique index.
            // All inline constraint kinds -> classified constraint entries.
            for column in &create.columns {
                let col = column.name.value.to_lowercase();
                for opt in &column.options {
                    let explicit = |name: &Option<sqlparser::ast::Ident>| {
                        name.as_ref()
                            .or(opt.name.as_ref())
                            .map(|i| i.value.to_lowercase())
                    };
                    let classified = match &opt.option {
                        ColumnOption::PrimaryKey(pk) => Some((
                            explicit(&pk.name).unwrap_or_else(|| format!("{table}_pkey")),
                            ConstraintKind::Unique,
                        )),
                        ColumnOption::Unique(u) => Some((
                            explicit(&u.name).unwrap_or_else(|| format!("{table}_{col}_key")),
                            ConstraintKind::Unique,
                        )),
                        ColumnOption::ForeignKey(fk) => Some((
                            explicit(&fk.name).unwrap_or_else(|| format!("{table}_{col}_fkey")),
                            ConstraintKind::ForeignKey,
                        )),
                        ColumnOption::Check(c) => Some((
                            explicit(&c.name).unwrap_or_else(|| format!("{table}_{col}_check")),
                            ConstraintKind::Check,
                        )),
                        _ => None,
                    };
                    if let Some((name, kind)) = classified {
                        push_constraint(constraints, &table, name, kind);
                    }
                    if matches!(
                        opt.option,
                        ColumnOption::PrimaryKey(_) | ColumnOption::Unique(_)
                    ) {
                        push_entry(
                            entries,
                            IndexEntry {
                                table: table.clone(),
                                columns: vec![col.clone()],
                                unique: true,
                                name: None,
                                partial: false,
                            },
                        );
                    }
                }
            }
            // Table-level constraints.
            for constraint in &create.constraints {
                apply_table_constraint(entries, constraints, &table, constraint);
            }
        }
        Statement::AlterTable(alter) => {
            let Some(table) = last_ident(&alter.name) else {
                return;
            };
            for op in &alter.operations {
                match op {
                    AlterTableOperation::AddConstraint { constraint, .. } => {
                        apply_table_constraint(entries, constraints, &table, constraint);
                    }
                    AlterTableOperation::DropConstraint { name, .. } => {
                        let name = name.value.to_lowercase();
                        entries.retain(|e| e.name.as_deref() != Some(name.as_str()));
                        constraints.retain(|c| !(c.table == table && c.name == name));
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
                        constraints.retain(|c| c.name != name);
                    }
                }
            }
            ObjectType::Table => {
                for name in names {
                    if let Some(table) = last_ident(name) {
                        entries.retain(|e| e.table != table);
                        constraints.retain(|c| c.table != table);
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// Apply one table-level constraint: `UNIQUE` / `PRIMARY KEY` become unique
/// index entries, and every classifiable kind (unique, primary key, foreign
/// key, check) becomes a classified constraint entry. Unnamed table-level
/// `CHECK` constraints are skipped — Postgres derives their name from the
/// columns referenced in the expression.
fn apply_table_constraint(
    entries: &mut Vec<IndexEntry>,
    constraints: &mut Vec<ConstraintEntry>,
    table: &str,
    constraint: &TableConstraint,
) {
    match constraint {
        TableConstraint::PrimaryKey(pk) => {
            let columns: Vec<String> = pk.columns.iter().map(index_column_name).collect();
            let name = pk.name.as_ref().map(|i| i.value.to_lowercase());
            push_constraint(
                constraints,
                table,
                name.clone().unwrap_or_else(|| format!("{table}_pkey")),
                ConstraintKind::Unique,
            );
            push_entry(
                entries,
                IndexEntry {
                    table: table.to_string(),
                    columns,
                    unique: true,
                    name,
                    partial: false,
                },
            );
        }
        TableConstraint::Unique(u) => {
            let columns: Vec<String> = u.columns.iter().map(index_column_name).collect();
            let name = u.name.as_ref().map(|i| i.value.to_lowercase());
            let constraint_name = name
                .clone()
                .or_else(|| synthesized_name(table, &columns, "key"));
            if let Some(constraint_name) = constraint_name {
                push_constraint(constraints, table, constraint_name, ConstraintKind::Unique);
            }
            push_entry(
                entries,
                IndexEntry {
                    table: table.to_string(),
                    columns,
                    unique: true,
                    name,
                    partial: false,
                },
            );
        }
        TableConstraint::ForeignKey(fk) => {
            let columns: Vec<String> = fk.columns.iter().map(|i| i.value.to_lowercase()).collect();
            let constraint_name = fk
                .name
                .as_ref()
                .map(|i| i.value.to_lowercase())
                .or_else(|| synthesized_name(table, &columns, "fkey"));
            if let Some(constraint_name) = constraint_name {
                push_constraint(
                    constraints,
                    table,
                    constraint_name,
                    ConstraintKind::ForeignKey,
                );
            }
        }
        TableConstraint::Check(c) => {
            if let Some(name) = c.name.as_ref() {
                push_constraint(
                    constraints,
                    table,
                    name.value.to_lowercase(),
                    ConstraintKind::Check,
                );
            }
        }
        _ => {}
    }
}

/// Postgres' auto-name for an unnamed constraint/index: `{table}_{cols}_{suffix}`.
/// `None` when any key element is an expression (opaque) or the joined name
/// would exceed Postgres' 63-byte identifier limit (Postgres shortens such
/// names with heuristics this simulator does not replicate).
fn synthesized_name(table: &str, columns: &[String], suffix: &str) -> Option<String> {
    if columns.is_empty() || columns.iter().any(|c| c == OPAQUE_COLUMN) {
        return None;
    }
    let name = format!("{table}_{}_{suffix}", columns.join("_"));
    (name.len() <= 63).then_some(name)
}

fn push_constraint(
    constraints: &mut Vec<ConstraintEntry>,
    table: &str,
    name: String,
    kind: ConstraintKind,
) {
    // Postgres truncates identifiers beyond 63 bytes; skip rather than guess.
    if name.len() > 63 {
        return;
    }
    let entry = ConstraintEntry {
        table: table.to_string(),
        name,
        kind,
    };
    if !constraints.contains(&entry) {
        constraints.push(entry);
    }
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
        // must not be wrongly split into spurious statements.
        let c = cat(
            "CREATE FUNCTION f() RETURNS void AS $$ BEGIN CREATE INDEX ON t (a); END; $$ LANGUAGE plpgsql; \
             CREATE INDEX real_idx ON u (b);",
        );
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.entries[0].table, "u");
        assert_eq!(c.entries[0].name.as_deref(), Some("real_idx"));
    }

    #[test]
    fn unique_index_names_by_last_column() {
        let c = cat("CREATE UNIQUE INDEX idx_users_email ON users (email); \
             CREATE UNIQUE INDEX users_org_email ON users (org_id, email); \
             CREATE UNIQUE INDEX users_email_org ON users (email, org_id); \
             CREATE TABLE t (id uuid PRIMARY KEY, name varchar UNIQUE); \
             CREATE INDEX idx_users_email_nonunique ON users (email);");
        // Single-column, and a composite whose LAST column is `email`, both map
        // to `email` (mirrors lana's composite `constraint = "…"` usage).
        let mut names = c.unique_index_names("users", "email");
        names.sort();
        assert_eq!(names, vec!["idx_users_email", "users_org_email"]);
        // A composite whose last column is `org_id` maps to `org_id`, not email.
        assert_eq!(
            c.unique_index_names("users", "org_id"),
            vec!["users_email_org"]
        );
        // The non-unique index on email is not a constraint.
        assert!(
            !c.unique_index_names("users", "email")
                .contains(&"idx_users_email_nonunique".to_string())
        );
        // Inline unnamed `UNIQUE` / `PRIMARY KEY` are Postgres-named at runtime.
        assert!(c.unique_index_names("t", "name").is_empty());
        assert!(c.unique_index_names("t", "id").is_empty());
    }

    #[test]
    fn specializes_on_equality_prefix() {
        // (account_id, created_at, id) supports {account_id} + sort created_at.
        let c = cat("CREATE INDEX ON transfers (account_id, created_at DESC, id);");
        assert!(c.specializes("transfers", &["account_id".to_string()], "created_at"));
        // Equality set is order-insensitive within the prefix.
        let c2 = cat("CREATE INDEX ON t (a, b, created_at, id);");
        assert!(c2.specializes("t", &["b".to_string(), "a".to_string()], "created_at"));
        // The equality being a leading prefix is sufficient — the sort column
        // need NOT immediately follow it (`account_id = $1` is still an index
        // scan; the COALESCE fallback would seq-scan).
        assert!(c.specializes("transfers", &["account_id".to_string()], "id"));
        // A bare `(filter)` index (no `(filter, sort)` composite) still
        // specializes the listing sorted by another column.
        let c4 = cat("CREATE INDEX ON transfers (account_id);");
        assert!(c4.specializes("transfers", &["account_id".to_string()], "created_at"));
        // No index covering the equality column → both plans seq-scan → fall back.
        assert!(!c4.specializes("transfers", &["reference".to_string()], "created_at"));
        // No equality filter: the sort column must lead an index.
        assert!(!c.specializes("transfers", &[], "created_at"));
        assert!(c.specializes("transfers", &[], "account_id"));
        // Partial indexes are ignored.
        let c3 = cat("CREATE INDEX ON t (a, created_at) WHERE deleted = FALSE;");
        assert!(!c3.specializes("t", &["a".to_string()], "created_at"));
    }

    // ── classified constraint catalog ───────────────────────────────────────

    #[test]
    fn constraints_inline_fk_and_pk_synthesized_names() {
        let c = cat("CREATE TABLE order_items (id uuid PRIMARY KEY, \
             order_id uuid NOT NULL REFERENCES orders(id));");
        assert_eq!(
            c.table_constraints("order_items"),
            vec![
                ("order_items_pkey".to_string(), ConstraintKind::Unique),
                (
                    "order_items_order_id_fkey".to_string(),
                    ConstraintKind::ForeignKey
                ),
            ]
        );
    }

    #[test]
    fn constraints_inline_unique_and_check_synthesized_names() {
        let c = cat("CREATE TABLE t (email varchar UNIQUE, a int CHECK (a > 0));");
        let cons = c.table_constraints("t");
        assert!(cons.contains(&("t_email_key".to_string(), ConstraintKind::Unique)));
        assert!(cons.contains(&("t_a_check".to_string(), ConstraintKind::Check)));
    }

    #[test]
    fn constraints_named_check_via_alter_then_drop() {
        let sql = "CREATE TABLE profiles (id uuid PRIMARY KEY, email varchar NOT NULL); \
             ALTER TABLE profiles ADD CONSTRAINT profiles_email_not_blank CHECK (email <> '');";
        let c = cat(sql);
        assert!(c.table_constraints("profiles").contains(&(
            "profiles_email_not_blank".to_string(),
            ConstraintKind::Check
        )));

        let dropped = cat(&format!(
            "{sql} ALTER TABLE profiles DROP CONSTRAINT profiles_email_not_blank;"
        ));
        assert!(
            !dropped
                .table_constraints("profiles")
                .iter()
                .any(|(_, k)| *k == ConstraintKind::Check)
        );
    }

    #[test]
    fn constraints_table_level_fk_named_and_unnamed() {
        let c = cat("CREATE TABLE t (a int, b int, \
             CONSTRAINT my_fk FOREIGN KEY (a) REFERENCES o(x), \
             FOREIGN KEY (b) REFERENCES o(y));");
        let cons = c.table_constraints("t");
        assert!(cons.contains(&("my_fk".to_string(), ConstraintKind::ForeignKey)));
        assert!(cons.contains(&("t_b_fkey".to_string(), ConstraintKind::ForeignKey)));
    }

    #[test]
    fn constraints_unnamed_table_level_check_skipped() {
        // Postgres derives the name from the expression's columns — not simulated.
        let c = cat("CREATE TABLE t (a int, CHECK (a > 0));");
        assert!(
            c.table_constraints("t")
                .iter()
                .all(|(_, k)| *k != ConstraintKind::Check)
        );
    }

    #[test]
    fn constraints_named_unique_index_and_drop_index() {
        let sql = "CREATE TABLE users (id uuid, email varchar); \
             CREATE UNIQUE INDEX idx_users_email ON users (email);";
        let c = cat(sql);
        assert!(
            c.table_constraints("users")
                .contains(&("idx_users_email".to_string(), ConstraintKind::Unique))
        );

        let dropped = cat(&format!("{sql} DROP INDEX idx_users_email;"));
        assert!(dropped.table_constraints("users").is_empty());
    }

    #[test]
    fn constraints_drop_table_removes_all() {
        let c = cat("CREATE TABLE t (id uuid PRIMARY KEY, o uuid REFERENCES x(id)); DROP TABLE t;");
        assert!(c.table_constraints("t").is_empty());
    }
}
