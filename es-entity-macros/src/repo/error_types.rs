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
/// matched column names)`. When the catalog has an entry for the table it is
/// authoritative: declared columns it does not constrain get no synthesized
/// `{table}_{col}_key`. Only without one is that convention assumed.
/// `matched` is the catalog's own columns when it
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
    let catalog_has_table = !constraints.is_empty();
    for col in &columns {
        let name = col.name().to_string();
        let name = if col.is_id() {
            catalog.pkey_constraint_name(table, &name)
        } else if catalog_has_table {
            continue;
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

/// Whether a constraint of [`enumerate_constraints`] is the primary key on
/// nothing but the id column, the one constraint a repo that generates its
/// own ids can never reject on.
pub(crate) fn is_id_pkey(
    opts: &RepositoryOptions,
    name: &str,
    kind: &ConstraintKind,
    col_names: &[String],
) -> bool {
    let Some(id) = opts.columns.column_enum_columns().find(|c| c.is_id()) else {
        return false;
    };
    matches!(kind, ConstraintKind::Unique)
        && col_names == [id.name().to_string()]
        && name
            == opts
                .index_catalog()
                .pkey_constraint_name(opts.table_name(), &id.name().to_string())
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
        let key_constraints = opts.ensure_key_constraint_names();
        let mut taken = HashSet::new();
        let mut variants = Vec::new();
        let mut check_variants: Vec<(syn::Ident, TokenStream)> = Vec::new();
        let mut classifiers = Vec::new();
        let mut set_values = Vec::new();
        let mut payloads = Vec::new();
        let mut foreign_keys = Vec::new();
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
                if opts.id_source().is_repo() {
                    continue;
                }
                let ty = cols[0].ty();
                taken.insert("Pkey".to_string());
                let description = format!("Violates the `{name}` primary key constraint.");
                let variant_def = quote! {
                    #[error("{0}")]
                    #[rejection(code = #name, description = #description)]
                    Pkey(#[source] es_entity::IdConflict<#ty>)
                };
                variants.push(variant_def);
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
            if matches!(kind, ConstraintKind::ForeignKey) {
                let context = format!(
                    "foreign key violation on {table}.{name} ({})",
                    col_names.join(", ")
                );
                foreign_keys.push(quote! { #name => Some(#context) });
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
            let is_check = matches!(kind, ConstraintKind::Check);
            let is_unique = matches!(kind, ConstraintKind::Unique);
            let kind = match kind {
                ConstraintKind::Unique => quote!(Unique),
                ConstraintKind::ForeignKey => quote!(ForeignKey),
                ConstraintKind::Check => quote!(Check),
            };
            let description = format!("Violates the `{name}` constraint.");
            let variant_def = quote! {
                #[error("{0}")]
                #[rejection(code = #name, description = #description)]
                #variant(#[source] es_entity::ConstraintConflict<#ty>)
            };
            if is_check || (is_unique && !key_constraints.contains(&name)) {
                check_variants.push((variant.clone(), variant_def.clone()));
            }
            variants.push(variant_def);
            classifiers.push(quote! {
                #name => Ok(Self::#variant(es_entity::ConstraintConflict::new(None, #table, #name, es_entity::ConstraintKind::#kind, source)))
            });
            set_values.push(quote! { Self::#variant(mut conflict) => { conflict.attempted = #attempted; Self::#variant(conflict) } });
        }
        for nested in opts.all_nested() {
            let path = match nested_constraint_violation_path(nested) {
                Ok(path) => path,
                Err(error) => return error.to_compile_error(),
            };
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
        let residuals = (!opts.ensure_keys().is_empty() && opts.ensure_can_reject()).then(|| {
            let residual = opts.ensure_rejection();
            let id_ty = opts.id();
            let code = format!("{}_keys_span_entities", table.replace('.', "_"));
            let outside_scope = (!opts.columns.scope_columns().is_empty()).then(|| {
                let code = format!("{}_keys_held_outside_scope", table.replace('.', "_"));
                quote! {
                    #[error("the unique keys are held by an entity outside the new entity's scope")]
                    #[rejection(code = #code, description = "The entity's unique keys are held by an entity in another scope.")]
                    KeysHeldOutsideScope { keys: Vec<&'static str> },
                }
            });
            let held_by_deleted = opts.delete.is_soft().then(|| {
                let code = format!("{}_keys_held_by_deleted", table.replace('.', "_"));
                quote! {
                    #[error("the unique keys are held by soft-deleted entities")]
                    #[rejection(code = #code, description = "The entity's unique keys are held only by soft-deleted entities.")]
                    KeysHeldByDeleted { ids: Vec<#id_ty>, keys: Vec<&'static str> },
                }
            });
            let definitions = check_variants.iter().map(|(_, definition)| definition);
            let idents: Vec<_> = check_variants.iter().map(|(ident, _)| ident).collect();
            quote! {
                #compose
                #[derive(Debug, Clone)]
                pub enum #residual {
                    #[error("the unique keys match more than one stored entity")]
                    #[rejection(code = #code, description = "The entity's unique keys are held by more than one stored entity.")]
                    KeysSpanEntities { ids: Vec<#id_ty>, keys: Vec<&'static str> },
                    #held_by_deleted
                    #outside_scope
                    #(#definitions),*
                }
                impl #residual {
                    #[doc(hidden)]
                    pub fn from_own(violation: #cv) -> Result<Self, #cv> {
                        match violation {
                            #(#cv::#idents(conflict) => Ok(Self::#idents(conflict)),)*
                            other => Err(other),
                        }
                    }
                    #[doc(hidden)]
                    pub fn narrow_fallback(
                        fail: es_entity::RepoWriteError<#cv>,
                        key_conflict: &'static str,
                    ) -> es_entity::RepoWriteError<Self> {
                        match fail.rejected() {
                            Ok(violation) => match Self::from_own(violation) {
                                Ok(rejection) => es_entity::errlanes::Fail::Rejected(rejection),
                                Err(_) => es_entity::errlanes::Transient::new(
                                    es_entity::errlanes::TransientKind::OptimisticConflict,
                                )
                                .with_context(key_conflict)
                                .into(),
                            },
                            Err(fault) => es_entity::errlanes::Fail::from(fault),
                        }
                    }
                    #[doc(hidden)]
                    pub fn narrow_write(
                        fail: es_entity::RepoWriteError<#cv>,
                    ) -> es_entity::RepoWriteError<Self> {
                        match fail.rejected() {
                            Ok(violation) => match Self::from_own(violation) {
                                Ok(rejection) => es_entity::errlanes::Fail::Rejected(rejection),
                                Err(other) => es_entity::errlanes::Fatal::invariant(
                                    "a unique constraint conflicted despite ON CONFLICT DO NOTHING",
                                )
                                .with_context(other.constraint_name().to_owned())
                                .into(),
                            },
                            Err(fault) => es_entity::errlanes::Fail::from(fault),
                        }
                    }
                }
            }
        });
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
                pub fn foreign_key_context(name: &str) -> Option<&'static str> {
                    match name { #(#foreign_keys,)* _ => None }
                }
                #[doc(hidden)]
                pub fn with_attempted(self, values: #values) -> Self {
                    match self { #(#set_values,)* #[allow(unreachable_patterns)] other => other }
                }
                #pkey_from_database
            }
            #write_error
            #residuals
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
/// The path of a nested child's generated `{Entity}ConstraintViolation`: the
/// child repo's own path with its last segment replaced. The violation enum is
/// always `pub`, so it can be named in a public signature even when the child
/// repo type is not.
pub(crate) fn nested_constraint_violation_path(
    nested: &super::options::RepoField,
) -> Result<syn::Path, syn::Error> {
    let syn::Type::Path(type_path) = &nested.ty else {
        return Err(syn::Error::new_spanned(
            &nested.ty,
            "nested repository must have a named type",
        ));
    };
    let mut path = type_path.path.clone();
    let entity = nested
        .entity
        .clone()
        .or_else(|| derive_entity_from_repo_type(&nested.ty))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &nested.ty,
                "cannot infer nested entity; specify entity explicitly",
            )
        })?;
    let last = path.segments.last_mut().unwrap();
    last.ident = format_ident!("{entity}ConstraintViolation");
    last.arguments = syn::PathArguments::None;
    Ok(path)
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
            #[es_repo(entity = "SharedId", id(source = caller), migrations_dir = "tests/fixtures/shared_id")]
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
        assert!(variants.iter().all(|v| v.ident != "IdFkey"));
        for expected in ["IdCheck", "IdKey"] {
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

    fn violation_variants(input: syn::DeriveInput, enum_name: &str) -> Vec<String> {
        use darling::FromDeriveInput;
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        let generated: syn::File = syn::parse2(ErrorTypes::new(&opts).generate()).unwrap();
        let mut names: Vec<_> = generated
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) if item.ident == enum_name => {
                    Some(item.variants.iter().map(|v| v.ident.to_string()))
                }
                _ => None,
            })
            .expect("generated rejection enum")
            .collect();
        names.sort();
        names
    }

    #[test]
    fn declared_column_absent_from_a_present_catalog_gets_no_key_variant() {
        let names = violation_variants(
            syn::parse_quote! {
                #[es_repo(
                    entity = "Gadget",
                    id(source = caller),
                    migrations_dir = "tests/fixtures/catalog_authoritative",
                    columns(name(ty = "String"), note(ty = "String"))
                )]
                struct Gadgets { pool: sqlx::PgPool }
            },
            "GadgetConstraintViolation",
        );
        assert_eq!(names, ["NameKey", "Pkey"]);
    }

    #[test]
    fn declared_column_without_a_catalog_keeps_the_synthesized_key_variant() {
        let names = violation_variants(
            syn::parse_quote! {
                #[es_repo(entity = "Gizmo", columns(note(ty = "String")))]
                struct Gizmos { pool: sqlx::PgPool }
            },
            "GizmoConstraintViolation",
        );
        assert!(names.contains(&"NoteKey".to_string()), "{names:?}");
    }
}
