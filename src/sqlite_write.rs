//! Write helpers called by the SQLite repository macros.
//!
//! The Postgres generators fold the index-row write, the events insert and the
//! snapshot upsert into one statement (data-modifying CTEs fed by `UNNEST`ed
//! arrays). SQLite has neither, so the SQLite generators run the index-row
//! statement themselves and hand the rest to these helpers, which issue plain
//! multi-row `INSERT`s inside the caller's transaction. Table names come from
//! the macro (never from callers); every value is bound.

use chrono::{DateTime, Utc};
use sqlx::{Arguments, AssertSqlSafe};

use crate::{ContextData, db};

pub use crate::sqlite_sql::expand_list_param;

/// Rows per multi-row `INSERT`. Each events row binds at most 6 values, well
/// under SQLite's default limit of 32766 bound parameters.
const CHUNK: usize = 500;

/// The timestamp a write records: the operation's cached/artificial time when
/// it has one, else the wall clock — what Postgres' `NOW()` would give.
pub fn resolve_now(now: Option<DateTime<Utc>>) -> DateTime<Utc> {
    now.unwrap_or_else(Utc::now)
}

/// Inserts one entity's new events with sequences `offset + 1 ..`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_entity_events<Id>(
    conn: &mut db::Connection,
    events_table: &str,
    event_ctx: bool,
    now: DateTime<Utc>,
    id: &Id,
    offset: usize,
    types: &[String],
    events: &[serde_json::Value],
    contexts: Option<&[ContextData]>,
) -> Result<(), sqlx::Error>
where
    Id: for<'q> sqlx::Encode<'q, db::Db> + sqlx::Type<db::Db> + Sync,
{
    let ids = vec![id; events.len()];
    let sequences: Vec<i32> = (offset + 1..)
        .take(events.len())
        .map(|i| i as i32)
        .collect();
    insert_events(
        conn,
        events_table,
        event_ctx,
        now,
        &ids,
        &sequences,
        types,
        events,
        contexts,
    )
    .await
}

/// Inserts events for many entities: parallel slices, one entry per event.
#[allow(clippy::too_many_arguments)]
pub async fn insert_events<Id>(
    conn: &mut db::Connection,
    events_table: &str,
    event_ctx: bool,
    now: DateTime<Utc>,
    ids: &[&Id],
    sequences: &[i32],
    types: &[String],
    events: &[serde_json::Value],
    contexts: Option<&[ContextData]>,
) -> Result<(), sqlx::Error>
where
    Id: for<'q> sqlx::Encode<'q, db::Db> + sqlx::Type<db::Db> + Sync,
{
    let n = events.len();
    let mut start = 0;
    while start < n {
        let end = (start + CHUNK).min(n);
        let row = if event_ctx {
            "(?, ?, ?, ?, ?, ?)"
        } else {
            "(?, ?, ?, ?, ?)"
        };
        let ctx_col = if event_ctx { ", context" } else { "" };
        let values = vec![row; end - start].join(", ");
        let sql = format!(
            "INSERT INTO {events_table} (id, recorded_at, sequence, event_type, event{ctx_col}) VALUES {values}"
        );
        let mut args = db::Arguments::default();
        for i in start..end {
            args.add(ids[i]).map_err(sqlx::Error::Encode)?;
            args.add(now).map_err(sqlx::Error::Encode)?;
            args.add(sequences[i]).map_err(sqlx::Error::Encode)?;
            args.add(types[i].as_str()).map_err(sqlx::Error::Encode)?;
            args.add(&events[i]).map_err(sqlx::Error::Encode)?;
            if event_ctx {
                args.add(contexts.and_then(|c| c.get(i)))
                    .map_err(sqlx::Error::Encode)?;
            }
        }
        sqlx::query_with(AssertSqlSafe(sql), args)
            .execute(&mut *conn)
            .await?;
        start = end;
    }
    Ok(())
}

/// Inserts forgettable payload rows. With `upsert`, a conflicting
/// `(entity_id, sequence)` row has its payload replaced (a re-snapshot
/// overwriting the reserved `sequence = 0` row).
pub async fn insert_payloads<Id>(
    conn: &mut db::Connection,
    table: &str,
    ids: &[&Id],
    sequences: &[i32],
    payloads: &[serde_json::Value],
    upsert: bool,
) -> Result<(), sqlx::Error>
where
    Id: for<'q> sqlx::Encode<'q, db::Db> + sqlx::Type<db::Db> + Sync,
{
    let n = payloads.len();
    let conflict = if upsert {
        " ON CONFLICT (entity_id, sequence) DO UPDATE SET payload = excluded.payload"
    } else {
        ""
    };
    let mut start = 0;
    while start < n {
        let end = (start + CHUNK).min(n);
        let values = vec!["(?, ?, ?)"; end - start].join(", ");
        let sql =
            format!("INSERT INTO {table} (entity_id, sequence, payload) VALUES {values}{conflict}");
        let mut args = db::Arguments::default();
        for i in start..end {
            args.add(ids[i]).map_err(sqlx::Error::Encode)?;
            args.add(sequences[i]).map_err(sqlx::Error::Encode)?;
            args.add(&payloads[i]).map_err(sqlx::Error::Encode)?;
        }
        sqlx::query_with(AssertSqlSafe(sql), args)
            .execute(&mut *conn)
            .await?;
        start = end;
    }
    Ok(())
}

/// What a SQLite constraint-violation message says about the constraint.
///
/// SQLite's `DatabaseError` reports neither `table()` nor `constraint()`;
/// the generated classifiers recover them from the message instead.
#[derive(Debug, PartialEq, Eq)]
pub enum ConstraintFailure {
    /// `UNIQUE constraint failed: tbl.a, tbl.b` (also primary keys).
    Unique { table: String, columns: Vec<String> },
    /// `UNIQUE constraint failed: index 'name'` (an expression index) or
    /// `CHECK constraint failed: name`: the constraint is named directly.
    Named(String),
    /// `FOREIGN KEY constraint failed`: SQLite does not say which one.
    ForeignKey,
    /// Anything else (not a constraint violation, `NOT NULL`, …).
    Other,
}

/// Parses a SQLite constraint-violation message.
pub fn constraint_failure(db_err: &dyn sqlx::error::DatabaseError) -> ConstraintFailure {
    parse_constraint_message(db_err.message())
}

fn parse_constraint_message(message: &str) -> ConstraintFailure {
    if let Some(rest) = message.strip_prefix("UNIQUE constraint failed: ") {
        if let Some(name) = rest
            .strip_prefix("index '")
            .and_then(|r| r.strip_suffix('\''))
        {
            return ConstraintFailure::Named(name.to_string());
        }
        let mut table = None;
        let mut columns = Vec::new();
        for qualified in rest.split(", ") {
            let Some((t, c)) = qualified.trim().split_once('.') else {
                return ConstraintFailure::Other;
            };
            table.get_or_insert_with(|| t.to_string());
            columns.push(c.to_string());
        }
        return match table {
            Some(table) => ConstraintFailure::Unique { table, columns },
            None => ConstraintFailure::Other,
        };
    }
    if let Some(name) = message.strip_prefix("CHECK constraint failed: ") {
        return ConstraintFailure::Named(name.trim().to_string());
    }
    if message.starts_with("FOREIGN KEY constraint failed") {
        return ConstraintFailure::ForeignKey;
    }
    ConstraintFailure::Other
}

/// The table of a SQLite unique violation (`None` for other errors), the
/// stand-in for Postgres' `DatabaseError::table()`.
pub fn constraint_table(db_err: &dyn sqlx::error::DatabaseError) -> Option<String> {
    match constraint_failure(db_err) {
        ConstraintFailure::Unique { table, .. } => Some(table),
        _ => None,
    }
}

/// Upserts one entity's head snapshot, keeping the newer of the stored and
/// the written row (by `sequence`), as the Postgres `snap` CTE does.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_snapshot<Id>(
    conn: &mut db::Connection,
    table: &str,
    now: DateTime<Utc>,
    id: &Id,
    sequence: i32,
    fingerprint: i64,
    snapshot: &serde_json::Value,
    first_recorded_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error>
where
    Id: for<'q> sqlx::Encode<'q, db::Db> + sqlx::Type<db::Db> + Sync,
{
    let sql = format!(
        "INSERT INTO {table} (id, sequence, fingerprint, snapshot, first_recorded_at, recorded_at) \
         VALUES (?1, ?2, ?3, ?4, COALESCE(?5, ?6), ?6) \
         ON CONFLICT (id) DO UPDATE SET sequence = excluded.sequence, fingerprint = excluded.fingerprint, \
         snapshot = excluded.snapshot, first_recorded_at = excluded.first_recorded_at, recorded_at = excluded.recorded_at \
         WHERE excluded.sequence > {table}.sequence"
    );
    sqlx::query(AssertSqlSafe(sql))
        .bind(id)
        .bind(sequence)
        .bind(fingerprint)
        .bind(snapshot)
        .bind(first_recorded_at)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ConstraintFailure, parse_constraint_message};

    #[test]
    fn parses_sqlite_constraint_messages() {
        assert_eq!(
            parse_constraint_message(
                "UNIQUE constraint failed: user_events.id, user_events.sequence"
            ),
            ConstraintFailure::Unique {
                table: "user_events".to_string(),
                columns: vec!["id".to_string(), "sequence".to_string()],
            }
        );
        assert_eq!(
            parse_constraint_message("UNIQUE constraint failed: index 'idx_lower_email'"),
            ConstraintFailure::Named("idx_lower_email".to_string())
        );
        assert_eq!(
            parse_constraint_message("CHECK constraint failed: amount_positive"),
            ConstraintFailure::Named("amount_positive".to_string())
        );
        assert_eq!(
            parse_constraint_message("FOREIGN KEY constraint failed"),
            ConstraintFailure::ForeignKey
        );
        assert_eq!(
            parse_constraint_message("NOT NULL constraint failed: users.name"),
            ConstraintFailure::Other
        );
    }
}
