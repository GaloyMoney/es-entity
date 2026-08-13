use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

/// Postgres reports the bare table name in errors, so a schema-qualified
/// events table from the repo options is reduced to its last path component
/// before being compared.
fn bare_table_name(events_table_name: &str) -> &str {
    events_table_name
        .rsplit('.')
        .next()
        .expect("rsplit yields at least one element")
}

/// The `extract_concurrent_modification` helper, which turns a unique
/// violation into the caller's `ConcurrentModification` variant.
///
/// It is emitted separately from `persist_events` because the combined write
/// statements classify their own errors, yet the follow-up forgettable
/// payload inserts (which are plain statements) still need it.
pub fn extract_concurrent_modification_fn() -> TokenStream {
    let mut tokens = TokenStream::new();
    tokens.append_all(quote! {
        fn extract_concurrent_modification<T, __EsErr: From<sqlx::Error>>(
            res: Result<T, sqlx::Error>,
            concurrent_modification: __EsErr,
        ) -> Result<T, __EsErr> {
            match res {
                Ok(v) => Ok(v),
                Err(sqlx::Error::Database(ref db_err)) if db_err.is_unique_violation() => {
                    Err(concurrent_modification)
                }
                Err(e) => Err(__EsErr::from(e)),
            }
        }
    });
    tokens
}

/// The `map_err` closure for a combined write statement whose error type has
/// no `ConstraintViolation` variant (e.g. the generated `{Entity}ForgetError`).
/// Only the events-table unique violation is distinguished; everything else
/// stays `Sqlx`.
pub fn concurrent_modification_classifier(
    error: &syn::Ident,
    events_table_name: &str,
) -> TokenStream {
    let events_table = bare_table_name(events_table_name);
    quote! {
        |e| match &e {
            sqlx::Error::Database(db_err)
                if db_err.is_unique_violation()
                    && db_err.table() == Some(#events_table) =>
            {
                #error::ConcurrentModification
            }
            _ => #error::Sqlx(e),
        }
    }
}

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
    let events_table = bare_table_name(events_table_name);
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
