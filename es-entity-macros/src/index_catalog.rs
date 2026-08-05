//! A best-effort PostgreSQL migration schema simulator that computes the final
//! set of physical composite indexes per table.
//!
//! This is the `IndexCatalog` source consumed by `#[derive(EsRepo)]` to decide
//! which `list_for_filters` combinations get a specialized sargable query (a
//! combo is specialized only when a matching composite index physically
//! exists). The catalog is derived from the committed migration `.sql` files at
//! macro-expansion time — the migrations directory *is* the source of truth
//! (precedent: `sqlx::migrate!`), so codegen is a deterministic function of the
//! checkout.
//!
//! It is intentionally a targeted parser over the controlled migration subset
//! (`CREATE [UNIQUE] INDEX`, `CREATE TABLE` inline/table-level `PRIMARY KEY` /
//! `UNIQUE`, `ALTER TABLE ADD/DROP CONSTRAINT`, `DROP INDEX`, `DROP TABLE`), not
//! a full SQL grammar: unrecognized statements are skipped, and a test-DB
//! verification lint (see `tests/`) catches any gap by diffing the parsed
//! catalog against `pg_indexes`.

use std::path::Path;

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

        let mut entries: Vec<IndexEntry> = Vec::new();
        for (_, sql) in sorted {
            let stripped = strip_comments(sql);
            for statement in split_statements(&stripped) {
                apply_statement(&mut entries, &statement);
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

/// One token of the targeted DDL tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// An identifier / keyword / number / operator run (lowercased). Quoted
    /// identifiers are folded in as continuations, string literals collapse to
    /// the placeholder `''`.
    Word(String),
    /// A top-level comma.
    Comma,
    /// A balanced `( ... )` group.
    Group(Vec<Tok>),
}

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
            // Preserve string literals verbatim (may contain -- or /* etc.).
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

/// Split on top-level `;` (ignoring semicolons inside parens or string
/// literals).
fn split_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => {
                cur.push(c);
                i += 1;
                while i < chars.len() {
                    cur.push(chars[i]);
                    if chars[i] == '\'' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                i += 1;
                continue;
            }
            _ => {}
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn tokenize(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let (toks, _) = tokenize_from(&chars, 0, false);
    toks
}

fn tokenize_from(chars: &[char], mut i: usize, in_group: bool) -> (Vec<Tok>, usize) {
    let mut out: Vec<Tok> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<Tok>| {
        if !cur.is_empty() {
            out.push(Tok::Word(std::mem::take(cur)));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '(' => {
                flush(&mut cur, &mut out);
                let (group, next) = tokenize_from(chars, i + 1, true);
                out.push(Tok::Group(group));
                i = next;
            }
            ')' if in_group => {
                flush(&mut cur, &mut out);
                return (out, i + 1);
            }
            ',' => {
                flush(&mut cur, &mut out);
                out.push(Tok::Comma);
                i += 1;
            }
            '"' => {
                // Quoted identifier: fold into the current word (so
                // `public."My_Tbl"` becomes one `public.my_tbl` token).
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    cur.push(chars[i].to_ascii_lowercase());
                    i += 1;
                }
                i += 1; // closing quote
            }
            '\'' => {
                flush(&mut cur, &mut out);
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' {
                        i += 1;
                        if i < chars.len() && chars[i] == '\'' {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                out.push(Tok::Word("''".to_string()));
            }
            c if c.is_whitespace() => {
                flush(&mut cur, &mut out);
                i += 1;
            }
            c => {
                cur.push(c.to_ascii_lowercase());
                i += 1;
            }
        }
    }
    flush(&mut cur, &mut out);
    (out, i)
}

fn strip_schema(word: &str) -> String {
    word.rsplit('.').next().unwrap_or(word).to_string()
}

fn as_word(tok: &Tok) -> Option<&str> {
    match tok {
        Tok::Word(w) => Some(w.as_str()),
        _ => None,
    }
}

/// Extract the ordered plain column names from an index/constraint key-column
/// group, splitting on top-level commas and stripping `ASC`/`DESC`/`NULLS ...`.
fn columns_from_group(group: &[Tok]) -> Vec<String> {
    let mut columns = Vec::new();
    for item in split_items(group) {
        let has_group = item.iter().any(|t| matches!(t, Tok::Group(_)));
        match item.first() {
            Some(Tok::Word(w)) if !has_group => columns.push(strip_schema(w)),
            _ => columns.push(OPAQUE_COLUMN.to_string()),
        }
    }
    columns
}

/// Split a token slice on top-level commas.
fn split_items(toks: &[Tok]) -> Vec<Vec<Tok>> {
    let mut items = Vec::new();
    let mut cur = Vec::new();
    for tok in toks {
        if matches!(tok, Tok::Comma) {
            items.push(std::mem::take(&mut cur));
        } else {
            cur.push(tok.clone());
        }
    }
    if !cur.is_empty() {
        items.push(cur);
    }
    items
}

fn apply_statement(entries: &mut Vec<IndexEntry>, statement: &str) {
    let toks = tokenize(statement);
    let head: Vec<&str> = toks.iter().filter_map(as_word).take(2).collect();
    match head.as_slice() {
        ["create", "table"] => parse_create_table(entries, &toks),
        ["create", "unique"] | ["create", "index"] => parse_create_index(entries, &toks),
        ["alter", "table"] => parse_alter_table(entries, &toks),
        ["drop", "index"] => parse_drop_index(entries, &toks),
        ["drop", "table"] => parse_drop_table(entries, &toks),
        _ => {}
    }
}

fn parse_create_index(entries: &mut Vec<IndexEntry>, toks: &[Tok]) {
    let mut i = 1; // past `create`
    let unique = matches!(toks.get(i), Some(Tok::Word(w)) if w == "unique");
    if unique {
        i += 1;
    }
    // `index`
    if !matches!(toks.get(i), Some(Tok::Word(w)) if w == "index") {
        return;
    }
    i += 1;
    // optional CONCURRENTLY / IF NOT EXISTS
    while let Some(Tok::Word(w)) = toks.get(i) {
        if matches!(w.as_str(), "concurrently" | "if" | "not" | "exists") {
            i += 1;
        } else {
            break;
        }
    }
    // optional index name (anything before `on`)
    let mut name: Option<String> = None;
    if let Some(Tok::Word(w)) = toks.get(i)
        && w != "on"
    {
        name = Some(w.clone());
        i += 1;
    }
    // `on`
    if !matches!(toks.get(i), Some(Tok::Word(w)) if w == "on") {
        return;
    }
    i += 1;
    if matches!(toks.get(i), Some(Tok::Word(w)) if w == "only") {
        i += 1;
    }
    let table = match toks.get(i) {
        Some(Tok::Word(w)) => strip_schema(w),
        _ => return,
    };
    i += 1;
    if matches!(toks.get(i), Some(Tok::Word(w)) if w == "using") {
        i += 2; // `using` + method
    }
    // key column group
    let columns = match toks.get(i) {
        Some(Tok::Group(g)) => columns_from_group(g),
        _ => return,
    };
    let partial = toks[i..]
        .iter()
        .any(|t| matches!(t, Tok::Word(w) if w == "where"));
    push_entry(
        entries,
        IndexEntry {
            table,
            columns,
            unique,
            name,
            partial,
        },
    );
}

fn parse_create_table(entries: &mut Vec<IndexEntry>, toks: &[Tok]) {
    let mut i = 2; // past `create table`
    while let Some(Tok::Word(w)) = toks.get(i) {
        if matches!(w.as_str(), "if" | "not" | "exists") {
            i += 1;
        } else {
            break;
        }
    }
    let table = match toks.get(i) {
        Some(Tok::Word(w)) => strip_schema(w),
        _ => return,
    };
    i += 1;
    let body = match toks.get(i) {
        Some(Tok::Group(g)) => g,
        _ => return,
    };
    for item in split_items(body) {
        parse_table_item(entries, &table, &item);
    }
}

fn parse_table_item(entries: &mut Vec<IndexEntry>, table: &str, item: &[Tok]) {
    let words: Vec<&str> = item.iter().filter_map(as_word).collect();
    let first = match words.first() {
        Some(w) => *w,
        None => return,
    };
    // Table-level constraints.
    match first {
        "primary" | "unique" | "constraint" => {
            let (name, kind_at) = if first == "constraint" {
                (words.get(1).map(|s| s.to_string()), 2)
            } else {
                (None, 0)
            };
            let is_unique = matches!(words.get(kind_at), Some(&"primary") | Some(&"unique"));
            if !is_unique {
                return;
            }
            // First group in the item is the column list.
            if let Some(Tok::Group(g)) = item.iter().find(|t| matches!(t, Tok::Group(_))) {
                push_entry(
                    entries,
                    IndexEntry {
                        table: table.to_string(),
                        columns: columns_from_group(g),
                        unique: true,
                        name,
                        partial: false,
                    },
                );
            }
            return;
        }
        "foreign" | "check" | "exclude" | "like" => return,
        _ => {}
    }
    // Column definition: `<col> <type...> [PRIMARY KEY | UNIQUE]`. An inline
    // `UNIQUE` or `PRIMARY KEY` yields a single-column unique index. Guard
    // against `... REFERENCES other(col)` which is a FK, not a local unique.
    let col = strip_schema(first);
    let has_pk = words.windows(2).any(|w| w == ["primary", "key"]);
    let has_unique = words.contains(&"unique");
    let is_fk = words.contains(&"references");
    if (has_pk || has_unique) && !is_fk {
        push_entry(
            entries,
            IndexEntry {
                table: table.to_string(),
                columns: vec![col],
                unique: true,
                name: None,
                partial: false,
            },
        );
    }
}

fn parse_alter_table(entries: &mut Vec<IndexEntry>, toks: &[Tok]) {
    let mut i = 2; // past `alter table`
    if matches!(toks.get(i), Some(Tok::Word(w)) if w == "only") {
        i += 1;
    }
    let table = match toks.get(i) {
        Some(Tok::Word(w)) => strip_schema(w),
        _ => return,
    };
    i += 1;
    match toks.get(i) {
        Some(Tok::Word(w)) if w == "add" => {
            let mut j = i + 1;
            let mut name = None;
            if matches!(toks.get(j), Some(Tok::Word(w)) if w == "constraint") {
                name = toks.get(j + 1).and_then(as_word).map(|s| s.to_string());
                j += 2;
            }
            let is_unique =
                matches!(toks.get(j), Some(Tok::Word(w)) if w == "primary" || w == "unique");
            if !is_unique {
                return;
            }
            if let Some(Tok::Group(g)) = toks[j..].iter().find(|t| matches!(t, Tok::Group(_))) {
                push_entry(
                    entries,
                    IndexEntry {
                        table,
                        columns: columns_from_group(g),
                        unique: true,
                        name,
                        partial: false,
                    },
                );
            }
        }
        Some(Tok::Word(w)) if w == "drop" => {
            if matches!(toks.get(i + 1), Some(Tok::Word(w)) if w == "constraint") {
                let mut j = i + 2;
                while matches!(toks.get(j), Some(Tok::Word(w)) if w == "if" || w == "exists") {
                    j += 1;
                }
                if let Some(name) = toks.get(j).and_then(as_word) {
                    entries.retain(|e| e.name.as_deref() != Some(name));
                }
            }
        }
        _ => {}
    }
}

fn parse_drop_index(entries: &mut Vec<IndexEntry>, toks: &[Tok]) {
    let mut i = 2; // past `drop index`
    while matches!(toks.get(i), Some(Tok::Word(w)) if matches!(w.as_str(), "concurrently" | "if" | "exists"))
    {
        i += 1;
    }
    for name in split_items(&toks[i..])
        .iter()
        .filter_map(|item| item.first().and_then(as_word))
    {
        let name = strip_schema(name);
        entries.retain(|e| e.name.as_deref() != Some(name.as_str()));
    }
}

fn parse_drop_table(entries: &mut Vec<IndexEntry>, toks: &[Tok]) {
    let mut i = 2; // past `drop table`
    while matches!(toks.get(i), Some(Tok::Word(w)) if w == "if" || w == "exists") {
        i += 1;
    }
    if let Some(table) = toks.get(i).and_then(as_word) {
        let table = strip_schema(table);
        entries.retain(|e| e.table != table);
    }
}

fn push_entry(entries: &mut Vec<IndexEntry>, entry: IndexEntry) {
    if !entries.contains(&entry) {
        entries.push(entry);
    }
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
