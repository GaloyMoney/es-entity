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

impl<'a> ErrorTypes<'a> {
    pub fn new(opts: &'a RepositoryOptions) -> Self {
        Self { opts }
    }
    pub fn generate(&self) -> TokenStream {
        let opts = self.opts;
        let cv = opts.constraint_violation();
        let values = format_ident!("{}ConstraintValues", opts.entity());
        let table = opts.table_name();
        let catalog_table = table.to_lowercase();
        let catalog = opts.index_catalog();
        let columns: Vec<_> = opts.columns.column_enum_columns().collect();
        let mut constraints = catalog.table_constraints(table);
        // Keep conventional names where migrations are unavailable.
        for col in &columns {
            let name = col.name().to_string();
            let mut names = vec![format!("{table}_{name}_key")];
            if col.is_id() {
                names.push(format!("{table}_pkey"));
            }
            for name in names {
                if !constraints.iter().any(|(n, _)| *n == name) {
                    constraints.push((name, ConstraintKind::Unique));
                }
            }
        }
        let mut taken = HashSet::new();
        let mut variants = Vec::new();
        let mut classifiers = Vec::new();
        let mut set_values = Vec::new();
        let mut payloads = Vec::new();
        for (name, kind) in constraints {
            let variant = constraint_variant_ident(table, &name, &mut taken);
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
            let matched: Option<Vec<_>> = if col_names.is_empty() {
                None
            } else {
                col_names
                    .iter()
                    .map(|n| columns.iter().find(|c| c.name() == n).copied())
                    .collect()
            };
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
            variants.push(quote! {
                #[error("{0}")]
                #[rejection(code = #name)]
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
            variants.push(quote! { #[compose(flatten)] #name(#path) });
        }
        let fields: Vec<_> = columns.iter().map(|c| c.name()).collect();
        let types: Vec<_> = columns.iter().map(|c| c.ty()).collect();
        let write_error = write_error_type(opts, &cv);
        quote! {
            #(#payloads)*
            #[doc(hidden)]
            #[derive(Default)]
            pub struct #values { #(pub #fields: Option<#types>),* }
            #[es_entity::errlanes::compose]
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
            #[classify(transient(OptimisticConflict))]
            Conflict(sqlx::Error),
            #[classify(delegate)]
            Other(sqlx::Error),
        }

        impl std::fmt::Display for #write_error {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Constraint(e) => std::fmt::Display::fmt(e, f),
                    Self::Conflict(e) => write!(f, "optimistic conflict: {e}"),
                    Self::Other(e) => std::fmt::Display::fmt(e, f),
                }
            }
        }

        impl std::error::Error for #write_error {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                    Self::Constraint(e) => Some(e),
                    Self::Conflict(e) => Some(e),
                    Self::Other(e) => Some(e),
                }
            }
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
}
