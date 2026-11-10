//! Rewrites the Postgres dialect the generators emit into SQLite.
//!
//! The generators in this crate are shared in shape with `es-entity-macros`
//! and still build their SQL in Postgres syntax. Everything that is a pure
//! *syntactic* difference is handled here, once, at expansion time:
//!
//! - `$N` placeholders become `?N` (same numbering, so binds are unchanged);
//! - `::TYPE` / `::TYPE[]` casts are dropped (SQLite is dynamically typed);
//! - `NOW()` becomes an RFC 3339 UTC timestamp, the same text shape sqlx
//!   encodes a `DateTime<Utc>` as, so stored timestamps sort consistently;
//! - `IS [NOT] DISTINCT FROM` becomes `IS [NOT]`;
//! - `FOR UPDATE` is dropped (SQLite locks the whole database on write);
//! - `ILIKE` becomes `LIKE` (ASCII case-insensitive in SQLite).
//!
//! Single-quoted literals and double-quoted identifiers (which carry sqlx
//! type overrides such as `"entity_id!: Repo__Id"`) are copied untouched.
//! Structural differences (`UNNEST`, `ANY`, data-modifying CTEs) are not
//! handled here; the generators avoid emitting them for SQLite.

const NOW_SQL: &str = "strftime('%Y-%m-%dT%H:%M:%f+00:00','now')";

pub fn translate(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' => {
                let end = quoted_end(&chars, i);
                out.extend(&chars[i..end]);
                i = end;
            }
            '$' if chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => {
                out.push('?');
                i += 1;
            }
            ':' if chars.get(i + 1) == Some(&':') => {
                i += 2;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if chars.get(i) == Some(&'[') && chars.get(i + 1) == Some(&']') {
                    i += 2;
                }
            }
            _ if starts_word(&chars, i) => {
                if let Some(len) = keyword_at(&chars, i, "NOW()") {
                    out.push_str(NOW_SQL);
                    i += len;
                } else if let Some(len) = keyword_at(&chars, i, "IS NOT DISTINCT FROM") {
                    out.push_str("IS");
                    i += len;
                } else if let Some(len) = keyword_at(&chars, i, "IS DISTINCT FROM") {
                    out.push_str("IS NOT");
                    i += len;
                } else if let Some(len) = keyword_at(&chars, i, "FOR UPDATE") {
                    // Also swallow the whitespace that preceded it.
                    while out.ends_with(' ') {
                        out.pop();
                    }
                    i += len;
                } else if let Some(len) = keyword_at(&chars, i, "ILIKE") {
                    out.push_str("LIKE");
                    i += len;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Index one past the closing quote of the literal starting at `start`.
/// A doubled quote inside the literal is an escaped quote.
fn quoted_end(chars: &[char], start: usize) -> usize {
    let q = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == q {
            if chars.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn starts_word(chars: &[char], i: usize) -> bool {
    is_word_char(chars[i]) && (i == 0 || !is_word_char(chars[i - 1]))
}

/// Case-insensitive match of `kw` at `i`, with any run of whitespace in the
/// input matching a single space in `kw`, ending on a word boundary. Returns
/// the number of input chars consumed.
fn keyword_at(chars: &[char], i: usize, kw: &str) -> Option<usize> {
    let mut j = i;
    let mut kw_chars = kw.chars().peekable();
    while let Some(k) = kw_chars.next() {
        if k == ' ' {
            if !chars.get(j).is_some_and(|c| c.is_whitespace()) {
                return None;
            }
            while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                j += 1;
            }
            continue;
        }
        if !chars.get(j).is_some_and(|c| c.eq_ignore_ascii_case(&k)) {
            return None;
        }
        j += 1;
    }
    let last_is_word = kw.chars().last().is_some_and(is_word_char);
    if last_is_word && chars.get(j).is_some_and(|c| is_word_char(*c)) {
        return None;
    }
    Some(j - i)
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn placeholders_and_casts() {
        assert_eq!(
            translate("SELECT id FROM t WHERE a = $1 AND b = $12::INT AND c = $3::TEXT[]"),
            "SELECT id FROM t WHERE a = ?1 AND b = ?12 AND c = ?3"
        );
    }

    #[test]
    fn quoted_parts_untouched() {
        assert_eq!(
            translate(
                "SELECT i.id AS \"entity_id!: es_entity::Id\", 'a$1::x' FROM t WHERE x IS NOT DISTINCT FROM $1"
            ),
            "SELECT i.id AS \"entity_id!: es_entity::Id\", 'a$1::x' FROM t WHERE x IS ?1"
        );
    }

    #[test]
    fn now_distinct_for_update() {
        assert_eq!(
            translate("SELECT COALESCE($2, NOW()) FROM t WHERE a IS DISTINCT FROM b FOR UPDATE"),
            "SELECT COALESCE(?2, strftime('%Y-%m-%dT%H:%M:%f+00:00','now')) FROM t WHERE a IS NOT b"
        );
    }

    #[test]
    fn keywords_respect_word_boundaries() {
        assert_eq!(
            translate("SELECT snow() , xnow()"),
            "SELECT snow() , xnow()"
        );
        assert_eq!(translate("WHERE name ILIKE $1"), "WHERE name LIKE ?1");
    }
}
