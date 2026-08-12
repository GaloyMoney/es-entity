use proc_macro2::TokenStream;
use quote::quote;

/// The `map_err` closure for a combined index+events write statement.
///
/// A single statement can fail from either table, so classification switches
/// on `DatabaseError::table()` instead of on which statement failed:
///
/// - unique violation on the events table → `ConcurrentModification`
/// - classified violation elsewhere (the index table) → `ConstraintViolation`
/// - anything else (including events-table FK violations) → `Sqlx`
///
/// The events table name may be schema-qualified in the repo options;
/// Postgres reports the bare table name in errors, so only the last path
/// component is compared.
pub fn write_error_classifier(error: &syn::Ident, events_table_name: &str) -> TokenStream {
    let events_table = events_table_name
        .rsplit('.')
        .next()
        .expect("rsplit yields at least one element");
    quote! {
        |e| match &e {
            sqlx::Error::Database(db_err)
                if db_err.is_unique_violation()
                    && db_err.table() == Some(#events_table) =>
            {
                #error::ConcurrentModification
            }
            sqlx::Error::Database(db_err)
                if db_err.table() != Some(#events_table)
                    && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
            {
                #error::ConstraintViolation {
                    column: Self::map_constraint_column(db_err.constraint()),
                    value: es_entity::extract_constraint_value(db_err.as_ref()),
                    inner: e,
                }
            }
            _ => #error::Sqlx(e),
        }
    }
}
