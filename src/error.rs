//! Types for working with errors produced by es-entity.

use thiserror::Error;

/// Error type for entity hydration failures (reconstructing entities from events).
#[derive(Error, Debug)]
pub enum EntityHydrationError {
    #[error("EntityHydrationError - UninitializedFieldError: {0}")]
    UninitializedFieldError(#[from] derive_builder::UninitializedFieldError),
    #[error("EntityHydrationError - Deserialization: {0}")]
    EventDeserialization(#[from] serde_json::Error),
}

#[derive(Error, Debug)]
#[error("CursorDestructureError: couldn't turn {0} into {1}")]
pub struct CursorDestructureError(&'static str, &'static str);

impl From<(&'static str, &'static str)> for CursorDestructureError {
    fn from((name, variant): (&'static str, &'static str)) -> Self {
        Self(name, variant)
    }
}

#[doc(hidden)]
/// Extracts the conflicting value from a PostgreSQL constraint violation detail message.
///
/// PostgreSQL formats unique violation details as:
/// `Key (column)=(value) already exists.`
///
/// Returns `None` if the detail is missing or doesn't match the expected format.
///
/// **Security note:** the extracted value is attacker-influenced input that
/// was rejected by a unique constraint and may be PII (e.g. an email
/// address). Returning it to untrusted API clients enables user
/// enumeration; logging it may place PII in log pipelines.
pub fn parse_constraint_detail_value(detail: Option<&str>) -> Option<String> {
    let detail = detail?;
    let start = detail.find("=(")? + 2;
    let end = detail.rfind(") already")?;
    if start <= end {
        Some(detail[start..end].to_string())
    } else {
        None
    }
}

#[doc(hidden)]
/// Extracts the conflicting value from a database error's constraint violation.
///
/// Downcasts to [`sqlx::postgres::PgDatabaseError`], reads its `detail()`,
/// and parses the conflicting value.
///
/// **Security note:** see [`parse_constraint_detail_value`] — the returned
/// value may be PII and must not be exposed to untrusted clients.
pub fn extract_constraint_value(db_err: &dyn sqlx::error::DatabaseError) -> Option<String> {
    let pg_err = db_err.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    parse_constraint_detail_value(pg_err.detail())
}

/// The kind of database constraint behind a classified `ConstraintViolation`.
///
/// Returned by the generated `{Entity}Constraint::kind()` method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    Unique,
    ForeignKey,
    Check,
}

#[doc(hidden)]
/// `true` when the database error is a violation kind the generated repo
/// classifiers surface as `ConstraintViolation`: unique (SQLSTATE 23505),
/// foreign key (23503), or check (23514). `NOT NULL` (23502) and exclusion
/// (23P01) violations are not classified and surface as `Sqlx`.
pub fn is_classified_constraint_violation(db_err: &dyn sqlx::error::DatabaseError) -> bool {
    db_err.is_unique_violation() || db_err.is_foreign_key_violation() || db_err.is_check_violation()
}

#[doc(hidden)]
/// Wrapper used by generated code to format not-found values.
/// Prefers `Display` over `Debug` via inherent-vs-trait method resolution.
pub struct NotFoundValue<'a, T: ?Sized>(pub &'a T);

impl<T: std::fmt::Display + ?Sized> NotFoundValue<'_, T> {
    pub fn to_not_found_value(&self) -> String {
        self.0.to_string()
    }
}

#[doc(hidden)]
pub trait ToNotFoundValueFallback {
    fn to_not_found_value(&self) -> String;
}

impl<T: std::fmt::Debug + ?Sized> ToNotFoundValueFallback for NotFoundValue<'_, T> {
    fn to_not_found_value(&self) -> String {
        format!("{:?}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_simple_uuid_value() {
        let detail = Some("Key (id)=(550e8400-e29b-41d4-a716-446655440000) already exists.");
        assert_eq!(
            parse_constraint_detail_value(detail),
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }

    #[test]
    fn parse_string_value() {
        let detail = Some("Key (email)=(user@example.com) already exists.");
        assert_eq!(
            parse_constraint_detail_value(detail),
            Some("user@example.com".to_string())
        );
    }

    #[test]
    fn parse_composite_key_value() {
        let detail = Some("Key (tenant_id, email)=(abc, user@example.com) already exists.");
        assert_eq!(
            parse_constraint_detail_value(detail),
            Some("abc, user@example.com".to_string())
        );
    }

    #[test]
    fn parse_none_detail() {
        assert_eq!(parse_constraint_detail_value(None), None);
    }

    #[test]
    fn parse_unexpected_format() {
        let detail = Some("something unexpected");
        assert_eq!(parse_constraint_detail_value(detail), None);
    }

    #[test]
    fn parse_value_containing_parentheses() {
        let detail = Some("Key (name)=(foo (bar)) already exists.");
        assert_eq!(
            parse_constraint_detail_value(detail),
            Some("foo (bar)".to_string())
        );
    }

    #[test]
    fn parse_empty_value() {
        let detail = Some("Key (col)=() already exists.");
        assert_eq!(parse_constraint_detail_value(detail), Some("".to_string()));
    }

    #[test]
    fn not_found_value_uses_display_when_available() {
        #[allow(unused_imports)]
        use crate::ToNotFoundValueFallback;

        // String implements Display - should get clean output
        let val = "hello";
        assert_eq!(NotFoundValue(val).to_not_found_value(), "hello");

        // i32 implements Display
        let num = 42;
        assert_eq!(NotFoundValue(&num).to_not_found_value(), "42");
    }

    #[test]
    fn not_found_value_falls_back_to_debug() {
        use crate::ToNotFoundValueFallback;

        // A type with Debug but no Display
        #[derive(Debug)]
        #[allow(dead_code)]
        struct OnlyDebug(i32);

        let val = OnlyDebug(7);
        assert_eq!(NotFoundValue(&val).to_not_found_value(), "OnlyDebug(7)");
    }

    proptest! {
        /// The parser does byte-index arithmetic over attacker-controlled strings.
        /// It must never panic, and any value it returns must be an honest
        /// substring bounded by the real markers.
        #[test]
        fn constraint_detail_never_panics_and_is_honest(detail in ".*") {
            match parse_constraint_detail_value(Some(&detail)) {
                None => {}
                Some(v) => {
                    // Returned value is always a genuine substring of the input.
                    prop_assert!(detail.contains(&v));
                    // The markers that drove the indices must actually be present.
                    let start = detail.find("=(").expect("start marker present") + 2;
                    let end = detail
                        .rfind(") already")
                        .expect("end marker present");
                    prop_assert!(start <= end);
                    prop_assert_eq!(&detail[start..end], v.as_str());
                }
            }
        }

        #[test]
        fn constraint_detail_none_input_returns_none(_ in Just(())) {
            prop_assert_eq!(parse_constraint_detail_value(None), None);
        }
    }
}
