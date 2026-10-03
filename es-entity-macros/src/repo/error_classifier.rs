use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// Postgres reports the bare table name in errors, so a schema-qualified
/// events table from the repo options is reduced to its last path component
/// before being compared.
fn bare_table_name(events_table_name: &str) -> &str {
    events_table_name
        .rsplit('.')
        .next()
        .expect("rsplit yields at least one element")
}

/// The classifier helpers for the combined index+events write statements and
/// the column-less conflict paths, emitted once per repo so the call sites
/// are a bare `map_err`/`Self::` reference instead of a match rendered into
/// every write path.
pub struct ErrorClassifier<'a> {
    constraint_violation: syn::Ident,
    id_type: &'a syn::Ident,
    /// The index table's actual pkey constraint name — the catalog's own
    /// name when a migration names the primary key explicitly, the
    /// `{table}_pkey` convention otherwise. Resolved once here via
    /// [`crate::index_catalog::IndexCatalog::pkey_constraint_name`], the
    /// same lookup `ErrorTypes` uses to name the `Pkey` variant, so an
    /// events-table duplicate-id conflict (classified here) and an
    /// index-table one (classified via `#constraint_violation::from_database`)
    /// attribute to the same variant.
    index_pkey: String,

    events_table_name: &'a str,
    /// `classify_update_write` is emitted only where a write path actually
    /// calls it — an uncalled private helper is dead code, and consumers build
    /// with `-D warnings`. `classify_create_write` needs no such gate:
    /// `create` and `create_all` always issue the combined statement.
    needs_write_classifier: bool,
}

impl<'a> From<&'a RepositoryOptions> for ErrorClassifier<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        let table_name = opts.table_name();
        let id_column_name = opts
            .columns
            .column_enum_columns()
            .find(|c| c.is_id())
            .expect("an EsRepo entity always has exactly one id column")
            .name()
            .to_string();
        let index_pkey = opts
            .index_catalog()
            .pkey_constraint_name(table_name, &id_column_name);
        Self {
            constraint_violation: opts.constraint_violation(),
            id_type: opts.id(),
            index_pkey,

            events_table_name: opts.events_table_name(),
            needs_write_classifier: opts.columns.updates_needed() || opts.delete.is_soft(),
        }
    }
}

impl ToTokens for ErrorClassifier<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.append_all(create_write_classifier_fn(
            &self.constraint_violation,
            self.id_type,
            &self.index_pkey,
            self.events_table_name,
        ));
        if self.needs_write_classifier {
            tokens.append_all(update_write_classifier_fn(
                &self.constraint_violation,
                self.events_table_name,
            ));
        }
    }
}

/// Shared by both combined-write classifiers: a classified violation
/// reported against any table other than the events table is the index
/// table's, and maps straight from the constraint Postgres named.
///
/// The events table name may be schema-qualified in the repo options;
/// Postgres reports the bare table name in errors, so only the last path
/// component is compared.
fn index_violation_arm(constraint_violation: &syn::Ident, events_table: &str) -> TokenStream {
    quote! {
        sqlx::Error::Database(db_err)
            if db_err.table() != Some(#events_table)
                && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
        {
            let name = db_err.constraint().unwrap_or("unknown").to_owned();
            match #constraint_violation::from_database(e, &name) {
                Ok(rejection) => errlanes::Fail::Rejected(rejection),
                Err(source) => errlanes::Fatal::from_error(errlanes::FatalKind::Invariant, source).with_context(name).into(),
            }
        }
    }
}

/// Classifier for the create paths' combined index+events write statement
/// (`create` / `create_all`).
///
/// A brand-new entity's events always start at sequence 1, so a unique
/// violation on the events-table `(id, sequence)` primary key can only mean
/// the id already exists — a pre-existing row, a concurrent create, or an
/// intra-batch duplicate in `create_all`. That is semantically a duplicate
/// id, not a concurrent conflict, so it is a `Rejected` constraint
/// violation, not `Transient`.
///
/// Postgres executes the data-modifying CTE (index insert) and the main
/// statement (events insert) interleaved with no guaranteed ordering, so for
/// a duplicate id either table's constraint may fire first depending on the
/// chosen plan. Both are therefore classified identically, and both
/// attribute the id via `attempted_id` rather than trusting the database
/// message as a value — see `IdConflict`'s doc and the "robustness" section
/// of the handoff this implements:
///
/// - unique violation on the events table → attribute via `attempted_id`,
///   `Rejected(Pkey(IdConflict))` on a match, `Fatal(Invariant)` otherwise
/// - unique violation on the index table's own pkey constraint → the same
/// - any other classified violation on the index table → `Rejected` mapped
///   from the reported constraint (unchanged, via `from_database`)
/// - anything else → the central `sqlx::Error` classifier
///
/// `attempted_id` is a caller-supplied closure (not a value) because the
/// single-create and batch call sites recover the id completely differently:
/// a single create already knows it from its own input, while `create_all`
/// must parse the database's reported key and match it against the batch's
/// own ids — the closure defers that work until it is known to be needed at
/// all (and only invoked where a violation that could be a duplicate id
/// actually fired).
fn create_write_classifier_fn(
    constraint_violation: &syn::Ident,
    id_type: &syn::Ident,
    index_pkey: &str,
    events_table_name: &str,
) -> TokenStream {
    let events_table = bare_table_name(events_table_name);
    let index_arm = index_violation_arm(constraint_violation, events_table);
    let not_attributable_context = format!("{index_pkey}: conflicting id not attributable");
    quote! {
        #[inline(always)]
        fn classify_create_write(
            e: sqlx::Error,
            attempted_id: impl FnOnce(&dyn sqlx::error::DatabaseError) -> Option<#id_type>,
        ) -> es_entity::RepoWriteError<#constraint_violation> {
            match &e {
                sqlx::Error::Database(db_err)
                    if db_err.is_unique_violation()
                        && db_err.table() == Some(#events_table) =>
                {
                    match attempted_id(db_err.as_ref()) {
                        Some(id) => errlanes::Fail::Rejected(#constraint_violation::pkey_from_database(e, id)),
                        None => errlanes::Fatal::from_error(errlanes::FatalKind::Invariant, e)
                            .with_context(#not_attributable_context)
                            .into(),
                    }
                }
                sqlx::Error::Database(db_err)
                    if db_err.is_unique_violation()
                        && db_err.table() != Some(#events_table)
                        && db_err.constraint() == Some(#index_pkey) =>
                {
                    match attempted_id(db_err.as_ref()) {
                        Some(id) => errlanes::Fail::Rejected(#constraint_violation::pkey_from_database(e, id)),
                        None => errlanes::Fatal::from_error(errlanes::FatalKind::Invariant, e)
                            .with_context(#not_attributable_context)
                            .into(),
                    }
                }
                #index_arm
                _ => errlanes::Fail::from(e),
            }
        }
    }
}

/// Classifier for a combined index+events write statement on the
/// update/delete paths.
///
/// A single statement can fail from either table, so classification switches
/// on `DatabaseError::table()` instead of on which statement failed:
///
/// - unique violation on the events table → `Transient(OptimisticConflict)`
///   — the entity already has persisted events, so an events-table `(id,
///   sequence)` conflict genuinely means another writer claimed the next
///   sequence first
/// - classified violation elsewhere (the index table) → `Rejected`
/// - anything else (including events-table FK violations) → the central
///   `sqlx::Error` classifier
fn update_write_classifier_fn(
    constraint_violation: &syn::Ident,
    events_table_name: &str,
) -> TokenStream {
    let events_table = bare_table_name(events_table_name);
    let index_arm = index_violation_arm(constraint_violation, events_table);
    quote! {
        #[inline(always)]
        fn classify_update_write(
            e: sqlx::Error,
            context: impl Into<std::borrow::Cow<'static, str>>,
        ) -> es_entity::RepoWriteError<#constraint_violation> {
            match &e {
                sqlx::Error::Database(db_err)
                    if db_err.is_unique_violation()
                        && db_err.table() == Some(#events_table) =>
                {
                    errlanes::Fail::from(
                        errlanes::Transient::from_error(errlanes::TransientKind::OptimisticConflict, e)
                            .with_context(context)
                    )
                }
                #index_arm
                _ => errlanes::Fail::from(e),
            }
        }
    }
}

/// Classifies a possible conflict from a plain (non-combined) statement:
/// the column-less `persist_events` write paths and the forgettable-payload
/// inserts. Both share one events-table primary key shape `(id, sequence)`,
/// but only a conflict *on the events table* is a real race — a conflict on
/// any other table (e.g. the forgettable payloads table, keyed
/// `(entity_id, sequence)`) is a bug: that table's rows are only ever
/// written once, by the same statement that advanced the sequence.
///
/// Every call site passes its repo's `events_table_name()` unchanged, which
/// may be schema-qualified (`events_tbl = "schema.table"`); Postgres reports
/// only the bare table name in `DatabaseError::table()`, so the comparison
/// strips a schema prefix here — once, centrally — rather than requiring
/// every call site to remember to.
///
/// Returns `es_entity::RepoFault` unconditionally, not generic over a
/// constraint type: nothing a column-less statement can hit is ever a typed
/// rejection, so there is no `D` for it to carry. A call site whose own
/// return type is `RepoWriteError<D>` still propagates it with a bare `?` —
/// `From<Fault<S>> for Fail<D, L>` already exists for exactly this — and one
/// whose return type is `RepoFault` itself propagates it by identity.
pub fn classify_conflict_fn() -> TokenStream {
    quote! {
        #[inline(always)]
        fn classify_conflict<T>(
            res: Result<T, sqlx::Error>,
            events_table: &'static str,
            context: impl FnOnce() -> String,
        ) -> Result<T, es_entity::RepoFault> {
            let events_table = events_table.rsplit('.').next().unwrap_or(events_table);
            match res {
                Ok(v) => Ok(v),
                Err(e) if e.as_database_error().is_some_and(|db_err|
                    db_err.is_unique_violation() && db_err.table() == Some(events_table)) =>
                {
                    Err(errlanes::Fault::from(
                        errlanes::Transient::from_error(errlanes::TransientKind::OptimisticConflict, e)
                            .with_context(context())
                    ))
                }
                Err(e) if e.as_database_error().is_some_and(|db_err| db_err.is_unique_violation()) => {
                    Err(errlanes::Fault::from(errlanes::Fatal::from_error(errlanes::FatalKind::Invariant, e).with_context(context())))
                }
                Err(e) => Err(errlanes::Fault::from(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;

    use super::*;

    /// Regression for a misclassified events-table conflict on a
    /// schema-qualified `events_tbl`: `classify_conflict`'s generated body
    /// must strip a schema prefix off its runtime `events_table` argument
    /// before comparing it against `DatabaseError::table()`, which Postgres
    /// only ever reports bare. Without the strip, a unique violation on
    /// e.g. `schema.entity_events` never matches and is misclassified
    /// `Fatal` instead of `Transient` — so update/forget/persist_events
    /// conflicts on such a repo would never be retried.
    #[test]
    fn classify_conflict_strips_schema_prefix_before_comparing() {
        let output = classify_conflict_fn().into_token_stream().to_string();
        assert!(
            output.contains("events_table . rsplit ('.') . next ()"),
            "classify_conflict must strip a schema prefix off `events_table` \
             before comparing it to `db_err.table()`: {output}"
        );
    }
}
