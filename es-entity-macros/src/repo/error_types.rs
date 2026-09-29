use convert_case::{Case, Casing};
use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};

use std::collections::HashSet;

use super::options::RepositoryOptions;
use crate::index_catalog::ConstraintKind;

pub struct ErrorTypes<'a> {
    entity: &'a syn::Ident,
    column_enum: syn::Ident,
    constraint_enum: syn::Ident,
    constraint_violation: syn::Ident,
    table_name: &'a str,
    column_variants: Vec<ColumnVariant>,
    constraint_variants: Vec<ConstraintVariant>,
    nested: Vec<NestedErrorInfo>,
}

struct ColumnVariant {
    variant_name: syn::Ident,
    column_name: String,
    constraint_names: Vec<String>,
}

struct ConstraintVariant {
    variant_name: syn::Ident,
    constraint_name: String,
    kind: ConstraintKind,
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

struct NestedErrorInfo {
    child_repo_ty: syn::Type,
    variant_name: syn::Ident,
    /// When set, the child's `ConstraintViolation` is referenced by
    /// convention-based concrete name (e.g. `FooConstraintViolation`)
    /// instead of an associated type projection
    /// (`<RepoType as EsRepo>::ConstraintViolation`). This avoids generic
    /// params leaking into module-level error enums.
    nested_entity: Option<syn::Ident>,
}

impl NestedErrorInfo {
    fn constraint_violation_ty(&self) -> TokenStream {
        if let Some(entity) = &self.nested_entity {
            let ty = syn::Ident::new(&format!("{entity}ConstraintViolation"), Span::call_site());
            quote! { #ty }
        } else {
            let child_repo_ty = &self.child_repo_ty;
            quote! { <#child_repo_ty as es_entity::EsRepo>::ConstraintViolation }
        }
    }
}

impl<'a> ErrorTypes<'a> {
    pub fn new(opts: &'a RepositoryOptions) -> Self {
        let table_name = opts.table_name();
        // The physical index catalog (parsed from the migrations) supplies the
        // real names of any *named* unique index whose last key column is this
        // column (single-column, or a composite whose leading columns scope it),
        // replacing the former per-column `constraint = "…"` attribute. The
        // Postgres name convention below still covers unnamed inline `UNIQUE` /
        // `PRIMARY KEY` constraints, so error mapping keeps working with no
        // migrations.
        let catalog = opts.index_catalog();
        let column_variants: Vec<ColumnVariant> = opts
            .columns
            .column_enum_columns()
            .map(|col| {
                let col_name = col.name().to_string();
                let variant_name =
                    syn::Ident::new(&col_name.to_case(Case::UpperCamel), Span::call_site());
                let mut constraint_names = vec![format!("{table_name}_{col_name}_key")];
                if col.is_id() {
                    constraint_names.push(format!("{table_name}_pkey"));
                }
                constraint_names.extend(catalog.unique_index_names(table_name, &col_name));
                ColumnVariant {
                    variant_name,
                    column_name: col_name,
                    constraint_names,
                }
            })
            .collect();

        // The typed constraint enum: the declared columns' unique constraint
        // names (convention + catalog derived, so the enum works even with no
        // discoverable migrations) plus every classified constraint (unique /
        // foreign key / check) the catalog found on this table.
        let mut taken = HashSet::new();
        let mut seen_names = HashSet::new();
        let mut constraint_variants: Vec<ConstraintVariant> = Vec::new();
        let unique_seed = column_variants
            .iter()
            .flat_map(|v| v.constraint_names.iter())
            .map(|name| (name.clone(), ConstraintKind::Unique));
        for (constraint_name, kind) in unique_seed.chain(catalog.table_constraints(table_name)) {
            if !seen_names.insert(constraint_name.clone()) {
                continue;
            }
            let variant_name = constraint_variant_ident(table_name, &constraint_name, &mut taken);
            constraint_variants.push(ConstraintVariant {
                variant_name,
                constraint_name,
                kind,
            });
        }

        let type_param_idents: Vec<&syn::Ident> =
            opts.generics.type_params().map(|p| &p.ident).collect();

        let nested: Vec<NestedErrorInfo> = opts
            .all_nested()
            .map(|f| {
                let nested_entity = f.entity.clone().or_else(|| {
                    // Auto-derive entity name when the nested repo type uses parent generics.
                    // Conventions tried in order:
                    //   1. Strip "Repo" suffix: `ObligationRepo<Evt>` → "Obligation"
                    //   2. Singularize: `OrderItems<Evt>` → "OrderItem"
                    // Override with `#[es_repo(nested, entity = "...")]` if neither matches.
                    if !type_param_idents.is_empty()
                        && type_uses_any_generic(&f.ty, &type_param_idents)
                    {
                        derive_entity_from_repo_type(&f.ty)
                    } else {
                        None
                    }
                });
                NestedErrorInfo {
                    child_repo_ty: f.ty.clone(),
                    variant_name: f.nested_variant_name(),
                    nested_entity,
                }
            })
            .collect();

        Self {
            entity: opts.entity(),
            column_enum: opts.column_enum(),
            constraint_enum: syn::Ident::new(
                &format!("{}Constraint", opts.entity()),
                Span::call_site(),
            ),
            constraint_violation: opts.constraint_violation(),
            table_name,
            column_variants,
            constraint_variants,
            nested,
        }
    }

    pub fn generate(&self) -> TokenStream {
        let column_enum = self.generate_column_enum();
        let constraint_enum = self.generate_constraint_enum();
        let constraint_violation = self.generate_constraint_violation();

        quote! {
            #column_enum
            #constraint_enum
            #constraint_violation
        }
    }

    /// The typed constraint enum: one variant per constraint on the entity's
    /// table known at compile time — the declared columns' unique constraints
    /// plus every unique / foreign key / check constraint discoverable from
    /// the migrations — plus `Unknown`, reported when a violation names a
    /// constraint the catalog does not recognize.
    fn generate_constraint_enum(&self) -> TokenStream {
        let constraint_enum = &self.constraint_enum;
        let variants: Vec<_> = self
            .constraint_variants
            .iter()
            .map(|v| &v.variant_name)
            .collect();
        let from_name_arms: Vec<_> = self
            .constraint_variants
            .iter()
            .map(|v| {
                let variant = &v.variant_name;
                let name = &v.constraint_name;
                quote! { #name => Some(Self::#variant), }
            })
            .collect();
        let name_arms: Vec<_> = self
            .constraint_variants
            .iter()
            .map(|v| {
                let variant = &v.variant_name;
                let name = &v.constraint_name;
                quote! { Self::#variant => #name, }
            })
            .collect();
        let kind_arms: Vec<_> = self
            .constraint_variants
            .iter()
            .map(|v| {
                let variant = &v.variant_name;
                let kind = match v.kind {
                    ConstraintKind::Unique => quote! { es_entity::ConstraintKind::Unique },
                    ConstraintKind::ForeignKey => quote! { es_entity::ConstraintKind::ForeignKey },
                    ConstraintKind::Check => quote! { es_entity::ConstraintKind::Check },
                };
                quote! { Self::#variant => #kind, }
            })
            .collect();

        quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum #constraint_enum {
                #(#variants,)*
                /// A violation whose constraint name the migrations-derived
                /// catalog does not recognize.
                Unknown,
            }

            impl #constraint_enum {
                #[doc(hidden)]
                #[inline(always)]
                pub fn from_name(name: &str) -> Option<Self> {
                    match name {
                        #(#from_name_arms)*
                        _ => None,
                    }
                }

                /// The database constraint name.
                pub fn name(&self) -> &'static str {
                    match *self {
                        #(#name_arms)*
                        Self::Unknown => "unknown",
                    }
                }

                /// The kind of constraint (unique / foreign key / check), when
                /// known.
                pub fn kind(&self) -> es_entity::ConstraintKind {
                    match *self {
                        #(#kind_arms)*
                        Self::Unknown => es_entity::ConstraintKind::Unknown,
                    }
                }
            }

            impl std::fmt::Display for #constraint_enum {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(self.name())
                }
            }

            impl From<#constraint_enum> for &'static str {
                fn from(c: #constraint_enum) -> &'static str {
                    c.name()
                }
            }
        }
    }

    fn generate_column_enum(&self) -> TokenStream {
        let column_enum = &self.column_enum;
        let variants: Vec<_> = self
            .column_variants
            .iter()
            .map(|v| &v.variant_name)
            .collect();
        let display_arms: Vec<_> = self
            .column_variants
            .iter()
            .map(|v| {
                let variant = &v.variant_name;
                let name = &v.column_name;
                quote! { Self::#variant => write!(f, #name), }
            })
            .collect();

        quote! {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub enum #column_enum {
                #(#variants,)*
            }

            impl std::fmt::Display for #column_enum {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        #(#display_arms)*
                    }
                }
            }
        }
    }

    pub fn generate_map_constraint_fn(&self) -> TokenStream {
        let column_enum = &self.column_enum;
        let match_arms: Vec<_> = self
            .column_variants
            .iter()
            .flat_map(|v| {
                let variant = &v.variant_name;
                v.constraint_names.iter().map(move |name| {
                    quote! { Some(#name) => Some(#column_enum::#variant), }
                })
            })
            .collect();

        quote! {
            #[inline(always)]
            fn map_constraint_column(constraint: Option<&str>) -> Option<#column_enum> {
                match constraint {
                    #(#match_arms)*
                    _ => None,
                }
            }
        }
    }

    /// The one repo `Rejection`: `{Entity}ConstraintViolation`. A struct when
    /// the repo has no nested children; an enum (`Own` plus one tuple variant
    /// per nested field, each wrapping that child's own
    /// `ConstraintViolation`) when it does — a child's violation widens into
    /// the parent's via the generated `From` impl, so nested write paths only
    /// need `.map_err(errlanes::Fail::widen)`.
    fn generate_constraint_violation(&self) -> TokenStream {
        let cv = &self.constraint_violation;
        let column_enum = &self.column_enum;
        let constraint_enum = &self.constraint_enum;
        let table_name = self.table_name;
        let entity_name = self.entity.to_string();

        let own_fields = quote! {
            constraint: Option<#constraint_enum>,
            constraint_name: Option<String>,
            column: Option<#column_enum>,
            /// **Security note:** attacker-influenced input rejected by a
            /// unique constraint and may be PII (e.g. an email address).
            /// Exposing it to untrusted API clients enables user
            /// enumeration; logging it may place PII in log pipelines.
            /// `Display` never prints it.
            value: Option<String>,
        };

        if self.nested.is_empty() {
            quote! {
                #[derive(Debug, Clone)]
                pub struct #cv {
                    #own_fields
                }

                impl #cv {
                    #[doc(hidden)]
                    pub fn new_own(
                        constraint: Option<#constraint_enum>,
                        constraint_name: Option<String>,
                        column: Option<#column_enum>,
                        value: Option<String>,
                    ) -> Self {
                        Self { constraint, constraint_name, column, value }
                    }

                    pub fn constraint(&self) -> Option<#constraint_enum> {
                        self.constraint
                    }

                    pub fn constraint_name(&self) -> Option<&str> {
                        self.constraint_name.as_deref()
                    }

                    pub fn column(&self) -> Option<#column_enum> {
                        self.column
                    }

                    pub fn kind(&self) -> Option<es_entity::ConstraintKind> {
                        self.constraint.map(|c| c.kind())
                    }

                    /// **Security note:** may contain PII. See the field doc.
                    pub fn value(&self) -> Option<&str> {
                        self.value.as_deref()
                    }
                }

                impl std::fmt::Display for #cv {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(
                            f,
                            "constraint violation on {} ({})",
                            #table_name,
                            self.constraint_name.as_deref().unwrap_or("unknown"),
                        )
                    }
                }

                impl std::error::Error for #cv {}

                impl errlanes::Rejection for #cv {
                    type Code = #constraint_enum;

                    fn code(&self) -> Self::Code {
                        self.constraint.unwrap_or(#constraint_enum::Unknown)
                    }
                }

                impl errlanes::Liftable for #cv {
                    type Key = #constraint_enum;

                    fn key(&self) -> Option<Self::Key> {
                        self.constraint
                    }
                }
            }
        } else {
            let nested_variants: Vec<_> = self
                .nested
                .iter()
                .map(|n| {
                    let variant = &n.variant_name;
                    let ty = n.constraint_violation_ty();
                    quote! { #variant(#ty), }
                })
                .collect();
            let nested_display_arms: Vec<_> = self
                .nested
                .iter()
                .map(|n| {
                    let variant = &n.variant_name;
                    quote! { Self::#variant(e) => write!(f, "{}: {}", #entity_name, e), }
                })
                .collect();
            let nested_source_arms: Vec<_> = self
                .nested
                .iter()
                .map(|n| {
                    let variant = &n.variant_name;
                    quote! { Self::#variant(e) => Some(e), }
                })
                .collect();
            let nested_from_impls: Vec<_> = self
                .nested
                .iter()
                .map(|n| {
                    let variant = &n.variant_name;
                    let ty = n.constraint_violation_ty();
                    quote! {
                        impl From<#ty> for #cv {
                            fn from(e: #ty) -> Self {
                                Self::#variant(e)
                            }
                        }
                    }
                })
                .collect();

            quote! {
                #[derive(Debug, Clone)]
                pub enum #cv {
                    Own {
                        #own_fields
                    },
                    #(#nested_variants)*
                }

                impl #cv {
                    #[doc(hidden)]
                    pub fn new_own(
                        constraint: Option<#constraint_enum>,
                        constraint_name: Option<String>,
                        column: Option<#column_enum>,
                        value: Option<String>,
                    ) -> Self {
                        Self::Own { constraint, constraint_name, column, value }
                    }

                    pub fn constraint(&self) -> Option<#constraint_enum> {
                        match self {
                            Self::Own { constraint, .. } => *constraint,
                            _ => None,
                        }
                    }

                    pub fn constraint_name(&self) -> Option<&str> {
                        match self {
                            Self::Own { constraint_name, .. } => constraint_name.as_deref(),
                            _ => None,
                        }
                    }

                    pub fn column(&self) -> Option<#column_enum> {
                        match self {
                            Self::Own { column, .. } => *column,
                            _ => None,
                        }
                    }

                    pub fn kind(&self) -> Option<es_entity::ConstraintKind> {
                        self.constraint().map(|c| c.kind())
                    }

                    /// **Security note:** may contain PII. See the `Own`
                    /// field doc.
                    pub fn value(&self) -> Option<&str> {
                        match self {
                            Self::Own { value, .. } => value.as_deref(),
                            _ => None,
                        }
                    }
                }

                impl std::fmt::Display for #cv {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        match self {
                            Self::Own { constraint_name, .. } => write!(
                                f,
                                "constraint violation on {} ({})",
                                #table_name,
                                constraint_name.as_deref().unwrap_or("unknown"),
                            ),
                            #(#nested_display_arms)*
                        }
                    }
                }

                impl std::error::Error for #cv {
                    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                        match self {
                            Self::Own { .. } => None,
                            #(#nested_source_arms)*
                        }
                    }
                }

                impl errlanes::Rejection for #cv {
                    type Code = #constraint_enum;

                    fn code(&self) -> Self::Code {
                        self.constraint().unwrap_or(#constraint_enum::Unknown)
                    }
                }

                impl errlanes::Liftable for #cv {
                    type Key = #constraint_enum;

                    fn key(&self) -> Option<Self::Key> {
                        #cv::constraint(self)
                    }
                }

                #(#nested_from_impls)*
            }
        }
    }
}

/// Check if a type references any of the given idents (generic type params).
fn type_uses_any_generic(ty: &syn::Type, idents: &[&syn::Ident]) -> bool {
    let ts = ty.to_token_stream();
    token_stream_contains_any(ts, idents)
}

fn token_stream_contains_any(ts: proc_macro2::TokenStream, idents: &[&syn::Ident]) -> bool {
    for tt in ts {
        match tt {
            proc_macro2::TokenTree::Ident(ref i) => {
                if idents.iter().any(|id| *i == **id) {
                    return true;
                }
            }
            proc_macro2::TokenTree::Group(g) => {
                if token_stream_contains_any(g.stream(), idents) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Derive the entity name from a repo type using conventions:
/// 1. Strip `Repo` suffix: `ObligationRepo<Evt>` → `Obligation`
/// 2. Singularize: `OrderItems<Evt>` → `OrderItem`
///
/// Returns `None` if neither convention matches.
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
