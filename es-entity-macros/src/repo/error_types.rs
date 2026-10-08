use super::options::RepositoryOptions;
use crate::index_catalog::ConstraintKind;
use convert_case::{Case, Casing};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use std::collections::HashSet;

pub struct ErrorTypes<'a> {
    opts: &'a RepositoryOptions,
}
fn dedupe_variant_ident(candidate: String, taken: &mut HashSet<String>) -> syn::Ident {
    let mut candidate = candidate;
    if taken.contains(&candidate) {
        let base = candidate.clone();
        let mut n = 2;
        while taken.contains(&candidate) {
            candidate = format!("{base}{n}");
            n += 1;
        }
    }
    taken.insert(candidate.clone());
    syn::Ident::new(&candidate, Span::call_site())
}

/// A readable variant ident for a constraint name: the `{table}_` prefix is
/// stripped when present (`profiles_email_not_blank` → `EmailNotBlank`),
/// falling back to the full name when stripping yields an invalid or already
/// taken ident. Total: distinct constraint names that still camel-case
/// identically (e.g. `t_a_b_key` vs `t_a__b_key`) are disambiguated with a
/// deterministic numeric suffix rather than silently dropped from the enum.
fn constraint_variant_ident(
    table_name: &str,
    constraint_name: &str,
    taken: &mut HashSet<String>,
) -> syn::Ident {
    let stripped = constraint_name
        .strip_prefix(&format!("{table_name}_"))
        .unwrap_or(constraint_name);
    let mut candidate = stripped.to_case(Case::UpperCamel);
    if !candidate
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        || taken.contains(&candidate)
    {
        candidate = constraint_name.to_case(Case::UpperCamel);
    }
    if !candidate
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        // Degenerate quoted identifiers (e.g. a constraint named "2fa_check").
        candidate = format!("Constraint{candidate}");
    }
    dedupe_variant_ident(candidate, taken)
}

/// Every constraint on the entity's table, enumerated once: `(name, kind,
/// matched column names)`. `matched` is the catalog's own columns when it
/// has an entry for `name`, the conventional single-column match otherwise,
/// and empty for an opaque CHECK expression the catalog cannot resolve to
/// columns. The id column contributes exactly one entry, under the pkey's
/// actual name (the catalog's own, when a migration names the primary key
/// explicitly, the `{table}_pkey` convention otherwise) — never *also* the
/// generic `{table}_{col}_key` form, which would fabricate a second,
/// phantom "pkey" entry for a constraint name Postgres will never report.
///
/// Shared by `ErrorTypes` (building the constraint violation enum) and
/// `RepositoryOptions::update_can_reject` (deciding whether an update can
/// ever hit one of these), so both see exactly the same constraint set.
pub(crate) fn enumerate_constraints(
    opts: &RepositoryOptions,
) -> Vec<(String, ConstraintKind, Vec<String>)> {
    let table = opts.table_name();
    let catalog_table = table.to_lowercase();
    let catalog = opts.index_catalog();
    let columns: Vec<_> = opts.columns.column_enum_columns().collect();
    let mut constraints = catalog.table_constraints(table);
    for col in &columns {
        let name = col.name().to_string();
        let name = if col.is_id() {
            catalog.pkey_constraint_name(table, &name)
        } else {
            format!("{table}_{name}_key")
        };
        if !constraints.iter().any(|(n, _)| *n == name) {
            constraints.push((name, ConstraintKind::Unique));
        }
    }
    constraints
        .into_iter()
        .map(|(name, kind)| {
            let col_names = catalog
                .constraints
                .iter()
                .find(|e| e.table == catalog_table && e.name == name)
                .map(|e| e.columns.clone())
                .unwrap_or_else(|| {
                    columns
                        .iter()
                        .filter(|c| {
                            name == format!("{table}_{}_key", c.name())
                                || (c.is_id() && name == format!("{table}_pkey"))
                        })
                        .map(|c| c.name().to_string())
                        .collect()
                });
            (name, kind, col_names)
        })
        .collect()
}

impl<'a> ErrorTypes<'a> {
    pub fn new(opts: &'a RepositoryOptions) -> Self {
        Self { opts }
    }
    pub fn generate(&self) -> TokenStream {
        let opts = self.opts;
        let cv = opts.constraint_violation();
        let values = format_ident!("{}ConstraintValues", opts.entity());
        let table = opts.table_name();
        let columns: Vec<_> = opts.columns.column_enum_columns().collect();
        let constraints = enumerate_constraints(opts);
        let mut taken = HashSet::new();
        let mut variants = Vec::new();
        let mut classifiers = Vec::new();
        let mut set_values = Vec::new();
        let mut payloads = Vec::new();
        // A nested repository imports its whole constraint family under a
        // prefix, which the compose attribute lists as `Source as Prefix`.
        let mut imports = Vec::new();
        // `pkey_from_database`, once the id-only pkey constraint is found
        // below. A composite or non-id primary key leaves this `None` and
        // that constraint keeps `ConstraintConflict` like any other.
        let mut pkey_from_database: Option<TokenStream> = None;
        for (name, kind, col_names) in constraints {
            let matched: Option<Vec<_>> = if col_names.is_empty() {
                None
            } else {
                col_names
                    .iter()
                    .map(|n| columns.iter().find(|c| c.name() == n).copied())
                    .collect()
            };
            // The primary key on nothing but the id column: the id is always
            // known for a create (the write's own input, or — for a batch —
            // matched against the batch's own ids), so this gets the
            // non-optional `IdConflict<IdTy>` instead of `ConstraintConflict`.
            // A composite or non-id primary key falls through to the general
            // case below and keeps `ConstraintConflict` as before.
            if let Some(cols) = &matched
                && cols.len() == 1
                && cols[0].is_id()
                && matches!(kind, ConstraintKind::Unique)
                && name
                    == opts
                        .index_catalog()
                        .pkey_constraint_name(table, &cols[0].name().to_string())
            {
                let ty = cols[0].ty();
                taken.insert("Pkey".to_string());
                let description = format!("Violates the `{name}` primary key constraint.");
                variants.push(quote! {
                    #[error("{0}")]
                    #[rejection(code = #name, description = #description)]
                    Pkey(#[source] es_entity::IdConflict<#ty>)
                });
                // `kind` is always `Unique` here: both sources that feed
                // `constraints` (the catalog's own `PRIMARY KEY` handling,
                // and the gap-filling synthesis above) record a primary key
                // as `ConstraintKind::Unique` — there is no other kind a
                // primary key constraint could be.
                pkey_from_database = Some(quote! {
                    #[doc(hidden)]
                    pub fn pkey_from_database(source: sqlx::Error, attempted: #ty) -> Self {
                        Self::Pkey(es_entity::IdConflict::new(attempted, #table, #name, es_entity::ConstraintKind::Unique, source))
                    }
                });
                // No `classifiers` arm: `from_database` falls through to
                // `Err(source)` for this name, which is correct outside a
                // create (an UPDATE/DELETE pkey violation has no id to
                // attribute and becomes `Fatal(Invariant)`). No
                // `set_values` arm either: the id is already always
                // present, so `with_attempted` is the identity on `Pkey`
                // via the catch-all below.
                continue;
            }
            let variant = constraint_variant_ident(table, &name, &mut taken);
            let (ty, attempted) = match matched {
                Some(cols) if cols.len() == 1 => {
                    let col = cols[0];
                    let ty = col.ty();
                    let field = col.name();
                    (quote!(#ty), quote!(values.#field))
                }
                Some(cols) => {
                    let payload = format_ident!("{}{}Values", opts.entity(), variant);
                    let fields: Vec<_> = cols.iter().map(|c| c.name()).collect();
                    let types: Vec<_> = cols.iter().map(|c| c.ty()).collect();
                    payloads.push(quote! { #[derive(Debug, Clone)] pub struct #payload { #(pub #fields: #types),* } });
                    (
                        quote!(#payload),
                        quote!((|| Some(#payload { #(#fields: values.#fields?),* }))()),
                    )
                }
                None => (quote!(()), quote!(None)),
            };
            let kind = match kind {
                ConstraintKind::Unique => quote!(Unique),
                ConstraintKind::ForeignKey => quote!(ForeignKey),
                ConstraintKind::Check => quote!(Check),
            };
            let description = format!("Violates the `{name}` constraint.");
            variants.push(quote! {
                #[error("{0}")]
                #[rejection(code = #name, description = #description)]
                #variant(#[source] es_entity::ConstraintConflict<#ty>)
            });
            classifiers.push(quote! {
                #name => Ok(Self::#variant(es_entity::ConstraintConflict::new(None, #table, #name, es_entity::ConstraintKind::#kind, source)))
            });
            set_values.push(quote! { Self::#variant(mut conflict) => { conflict.attempted = #attempted; Self::#variant(conflict) } });
        }
        for nested in opts.all_nested() {
            let mut path = if let syn::Type::Path(p) = &nested.ty {
                p.path.clone()
            } else {
                return syn::Error::new_spanned(
                    &nested.ty,
                    "nested repository must have a named type",
                )
                .to_compile_error();
            };
            let entity = nested
                .entity
                .clone()
                .or_else(|| derive_entity_from_repo_type(&nested.ty));
            let Some(entity) = entity else {
                return syn::Error::new_spanned(
                    &nested.ty,
                    "cannot infer nested entity; specify entity explicitly",
                )
                .to_compile_error();
            };
            let last = path.segments.last_mut().unwrap();
            last.ident = format_ident!("{entity}ConstraintViolation");
            last.arguments = syn::PathArguments::None;
            let name = nested.nested_variant_name();
            imports.push(quote! { #path as #name });
        }
        let fields: Vec<_> = columns.iter().map(|c| c.name()).collect();
        let types: Vec<_> = columns.iter().map(|c| c.ty()).collect();
        let write_error = write_error_type(opts, &cv);
        let compose = if imports.is_empty() {
            quote!(#[es_entity::errlanes::compose])
        } else {
            quote!(#[es_entity::errlanes::compose(#(#imports),*)])
        };
        quote! {
            #(#payloads)*
            #[doc(hidden)]
            #[derive(Default)]
            pub struct #values { #(pub #fields: Option<#types>),* }
            #compose
            #[derive(Debug, Clone, es_entity::ConstraintRejection)]
            pub enum #cv { #(#variants),* }
            impl #cv {
                #[doc(hidden)]
                pub fn from_database(source: sqlx::Error, name: &str) -> Result<Self, sqlx::Error> {
                    match name { #(#classifiers,)* _ => Err(source) }
                }
                #[doc(hidden)]
                pub fn with_attempted(self, values: #values) -> Self {
                    match self { #(#set_values,)* #[allow(unreachable_patterns)] other => other }
                }
                #pkey_from_database
            }
            #write_error
        }
    }
}

/// `{Entity}WriteError`: the pattern `classify_create_write`/
/// `classify_update_write` already apply, made public and reusable. A
/// consumer hand-writing a query against the repo's own tables classifies
/// its `sqlx::Error` the same way a generated write op would:
/// `.classify::<{Entity}WriteError>()?`.
///
/// Unlike the create path's private classifier, an events-table unique
/// violation here is always `Conflict` (optimistic-conflict retry) — the
/// create-specific "duplicate id" reading of that same violation is
/// particular to a brand-new entity's first write and stays in
/// `classify_create_write`.
fn write_error_type(opts: &RepositoryOptions, constraint_violation: &syn::Ident) -> TokenStream {
    let write_error = opts.write_error();
    let events_table = opts
        .events_table_name()
        .rsplit('.')
        .next()
        .expect("rsplit yields at least one element");
    quote! {
        #[derive(Debug, es_entity::errlanes::Classify)]
        pub enum #write_error {
            #[classify(delegate)]
            Constraint(#constraint_violation),
            /// The events table's `(id, sequence)` unique constraint fired —
            /// another writer claimed the next sequence first.
            #[error("optimistic conflict")]
            #[classify(transient(OptimisticConflict))]
            Conflict(#[source] sqlx::Error),
            #[classify(delegate)]
            Other(sqlx::Error),
        }

        impl From<sqlx::Error> for #write_error {
            fn from(e: sqlx::Error) -> Self {
                match &e {
                    sqlx::Error::Database(db_err)
                        if db_err.is_unique_violation() && db_err.table() == Some(#events_table) =>
                    {
                        Self::Conflict(e)
                    }
                    sqlx::Error::Database(db_err)
                        if db_err.table() != Some(#events_table)
                            && es_entity::is_classified_constraint_violation(db_err.as_ref()) =>
                    {
                        let name = db_err.constraint().unwrap_or("unknown").to_owned();
                        match #constraint_violation::from_database(e, &name) {
                            Ok(rejection) => Self::Constraint(rejection),
                            Err(source) => Self::Other(source),
                        }
                    }
                    _ => Self::Other(e),
                }
            }
        }
    }
}
fn derive_entity_from_repo_type(ty: &syn::Type) -> Option<syn::Ident> {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        let name = segment.ident.to_string();

        // Convention 1: strip "Repo" suffix
        if let Some(entity_name) = name.strip_suffix("Repo")
            && !entity_name.is_empty()
        {
            return Some(syn::Ident::new(entity_name, segment.ident.span()));
        }

        // Convention 2: singularize plural name (e.g., OrderItems → OrderItem)
        let singular = pluralizer::pluralize(&name, 1, false);
        if singular != name {
            return Some(syn::Ident::new(&singular, segment.ident.span()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distinct SQL identifiers can collapse to the same Rust casing.
    #[test]
    fn dedupe_variant_ident_disambiguates_repeats() {
        let mut taken = HashSet::new();
        let first = dedupe_variant_ident("Items".to_string(), &mut taken);
        let second = dedupe_variant_ident("Items".to_string(), &mut taken);
        assert_eq!(first.to_string(), "Items");
        assert_eq!(second.to_string(), "Items2");
        assert_ne!(first, second);
    }

    #[test]
    fn dedupe_variant_ident_never_collides_with_reserved_unknown() {
        let mut taken = HashSet::new();
        taken.insert("Unknown".to_string());
        let variant = dedupe_variant_ident("Unknown".to_string(), &mut taken);
        assert_eq!(variant.to_string(), "Unknown2");
    }

    /// Exercise both the full-name fallback and the numeric suffix when
    /// earlier constraints have already claimed both candidate names.
    #[test]
    fn constraint_variant_ident_never_collides_with_reserved_unknown() {
        let mut taken = HashSet::new();
        taken.insert("Unknown".to_string());
        taken.insert("WidgetsUnknown".to_string());
        let variant = constraint_variant_ident("widgets", "widgets_unknown", &mut taken);
        assert_eq!(variant.to_string(), "WidgetsUnknown2");
    }
    #[test]
    fn id_foreign_key_and_redundant_unique_are_not_primary_keys() {
        use darling::FromDeriveInput;
        let input: syn::DeriveInput = syn::parse_quote! {
            #[es_repo(entity = "SharedId", migrations_dir = "tests/fixtures/shared_id")]
            struct SharedIds { pool: sqlx::PgPool }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        let generated: syn::File = syn::parse2(ErrorTypes::new(&opts).generate()).unwrap();
        let variants = generated
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) if item.ident == "SharedIdConstraintViolation" => {
                    Some(&item.variants)
                }
                _ => None,
            })
            .expect("generated rejection enum");
        assert_eq!(variants.iter().filter(|v| v.ident == "Pkey").count(), 1);
        let pkey = variants.iter().find(|v| v.ident == "Pkey").unwrap();
        assert!(quote!(#pkey).to_string().contains("IdConflict"));
        let generated_tokens = quote!(#generated).to_string();
        assert!(generated_tokens.contains("fn pkey_from_database"));
        assert!(generated_tokens.contains("\"shared_ids_actual_pk\""));
        for expected in ["IdFkey", "IdCheck", "IdKey"] {
            let variant = variants
                .iter()
                .find(|v| v.ident == expected)
                .expect(expected);
            let syn::Fields::Unnamed(fields) = &variant.fields else {
                panic!("constraint payload")
            };
            let ty = &fields.unnamed[0].ty;
            assert!(quote!(#ty).to_string().contains("ConstraintConflict"));
        }
    }
}
