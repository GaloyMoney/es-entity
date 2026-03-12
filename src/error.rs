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
pub fn extract_constraint_value(db_err: &dyn sqlx::error::DatabaseError) -> Option<String> {
    let pg_err = db_err.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    parse_constraint_detail_value(pg_err.detail())
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
}
