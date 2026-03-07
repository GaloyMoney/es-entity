use convert_case::{Case, Casing};
use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};

use super::options::{PostHydrateHookConfig, RepositoryOptions};

pub struct ErrorTypes<'a> {
    entity: &'a syn::Ident,
    column_enum: syn::Ident,
    create_error: syn::Ident,
    modify_error: syn::Ident,
    find_error: syn::Ident,
    query_error: syn::Ident,
    column_variants: Vec<ColumnVariant>,
    nested: Vec<NestedErrorInfo>,
    post_hydrate_hook: &'a Option<PostHydrateHookConfig>,
}

struct ColumnVariant {
    variant_name: syn::Ident,
    column_name: String,
    constraint_names: Vec<String>,
}

struct NestedErrorInfo {
    child_repo_ty: syn::Type,
    variant_name: syn::Ident,
    /// When set, error types are referenced by convention-based concrete names
    /// (e.g., `FooCreateError`) instead of associated type projections
    /// (`<RepoType as EsRepo>::CreateError`). This avoids generic params leaking
    /// into module-level error enums.
    nested_entity: Option<syn::Ident>,
}

impl NestedErrorInfo {
    fn create_error_ty(&self) -> TokenStream {
        if let Some(entity) = &self.nested_entity {
            let error_ident = syn::Ident::new(&format!("{entity}CreateError"), Span::call_site());
            quote! { #error_ident }
        } else {
            let child_repo_ty = &self.child_repo_ty;
            quote! { <#child_repo_ty as es_entity::EsRepo>::CreateError }
        }
    }

    fn modify_error_ty(&self) -> TokenStream {
        if let Some(entity) = &self.nested_entity {
            let error_ident = syn::Ident::new(&format!("{entity}ModifyError"), Span::call_site());
            quote! { #error_ident }
        } else {
            let child_repo_ty = &self.child_repo_ty;
            quote! { <#child_repo_ty as es_entity::EsRepo>::ModifyError }
        }
    }
}

impl<'a> ErrorTypes<'a> {
    pub fn new(opts: &'a RepositoryOptions) -> Self {
        let table_name = opts.table_name();
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
                if let Some(custom) = col.custom_constraint() {
                    constraint_names.push(custom.to_string());
                }
                ColumnVariant {
                    variant_name,
                    column_name: col_name,
                    constraint_names,
                }
            })
            .collect();

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
            create_error: opts.create_error(),
            modify_error: opts.modify_error(),
            find_error: opts.find_error(),
            query_error: opts.query_error(),
            column_variants,
            nested,
            post_hydrate_hook: &opts.post_hydrate_hook,
        }
    }

    pub fn generate(&self) -> TokenStream {
        let column_enum = self.generate_column_enum();
        let create_error = self.generate_create_error();
        let modify_error = self.generate_modify_error();
        let find_error = self.generate_find_error();
        let query_error = self.generate_query_error();

        quote! {
            #column_enum
            #create_error
            #modify_error
            #find_error
            #query_error
        }
    }

    pub fn generate_map_constraint_fn(&self) -> TokenStream {
        self.generate_map_constraint_column()
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

    fn generate_map_constraint_column(&self) -> TokenStream {
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

    fn generate_create_error(&self) -> TokenStream {
        let create_error = &self.create_error;
        let column_enum = &self.column_enum;
        let entity = self.entity;

        // Nested child variants
        let nested_variants: Vec<_> = self
            .nested
            .iter()
            .map(|n| {
                let variant = &n.variant_name;
                let child_error_ty = n.create_error_ty();
                quote! { #variant(#child_error_ty), }
            })
            .collect();
        let nested_display_arms: Vec<_> = self
            .nested
            .iter()
            .map(|n| {
                let variant = &n.variant_name;
                quote! { Self::#variant(e) => write!(f, "{}: {}", stringify!(#variant), e), }
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
                let child_error_ty = n.create_error_ty();
                quote! {
                    impl From<#child_error_ty> for #create_error {
                        fn from(e: #child_error_ty) -> Self {
                            Self::#variant(e)
                        }
                    }
                }
            })
            .collect();
        let nested_cm_checks: Vec<_> = self
            .nested
            .iter()
            .map(|n| {
                let variant = &n.variant_name;
                quote! { Self::#variant(e) => e.was_concurrent_modification(), }
            })
            .collect();
        let nested_dv_checks: Vec<_> = self
            .nested
            .iter()
            .map(|n| {
                let variant = &n.variant_name;
                quote! { Self::#variant(e) => e.duplicate_value(), }
            })
            .collect();

        let entity_name = entity.to_string();

        let (ph_variant, ph_display_arm, ph_source_arm) = if let Some(config) =
            &self.post_hydrate_hook
        {
            let error_ty = &config.error;
            (
                quote! { PostHydrateError(#error_ty), },
                quote! { Self::PostHydrateError(e) => write!(f, "{}CreateError - PostHydrateError: {}", #entity_name, e), },
                quote! { Self::PostHydrateError(e) => Some(e), },
            )
        } else {
            (quote! {}, quote! {}, quote! {})
        };

        quote! {
            #[derive(Debug)]
            pub enum #create_error {
                Sqlx(sqlx::Error),
                ConstraintViolation { column: Option<#column_enum>, value: Option<String>, inner: sqlx::Error },
                ConcurrentModification,
                HydrationError(es_entity::EntityHydrationError),
                PostPersistHookError(sqlx::Error),
                #ph_variant
                #(#nested_variants)*
            }

            impl std::fmt::Display for #create_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}CreateError - Sqlx: {}", #entity_name, e),
                        Self::ConstraintViolation { column, value, inner } => write!(f, "{}CreateError - ConstraintViolation({:?}, {:?}): {}", #entity_name, column, value, inner),
                        Self::ConcurrentModification => write!(f, "{}CreateError - ConcurrentModification", #entity_name),
                        Self::HydrationError(e) => write!(f, "{}CreateError - HydrationError: {}", #entity_name, e),
                        Self::PostPersistHookError(e) => write!(f, "{}CreateError - PostPersistHookError: {}", #entity_name, e),
                        #ph_display_arm
                        #(#nested_display_arms)*
                    }
                }
            }

            impl std::error::Error for #create_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::ConstraintViolation { inner, .. } => Some(inner),
                        Self::ConcurrentModification => None,
                        Self::HydrationError(e) => Some(e),
                        Self::PostPersistHookError(e) => Some(e),
                        #ph_source_arm
                        #(#nested_source_arms)*
                    }
                }
            }

            impl From<sqlx::Error> for #create_error {
                fn from(e: sqlx::Error) -> Self {
                    Self::Sqlx(e)
                }
            }

            impl From<es_entity::EntityHydrationError> for #create_error {
                fn from(e: es_entity::EntityHydrationError) -> Self {
                    Self::HydrationError(e)
                }
            }

            #(#nested_from_impls)*

            impl #create_error {
                pub fn was_concurrent_modification(&self) -> bool {
                    match self {
                        Self::ConcurrentModification => true,
                        #(#nested_cm_checks)*
                        _ => false,
                    }
                }

                pub fn was_duplicate(&self) -> bool {
                    matches!(self, Self::ConstraintViolation { .. })
                }

                pub fn was_duplicate_by(&self, column: #column_enum) -> bool {
                    matches!(self, Self::ConstraintViolation { column: Some(c), .. } if *c == column)
                }

                pub fn duplicate_value(&self) -> Option<&str> {
                    match self {
                        Self::ConstraintViolation { value: Some(v), .. } => Some(v.as_str()),
                        #(#nested_dv_checks)*
                        _ => None,
                    }
                }
            }
        }
    }

    fn generate_modify_error(&self) -> TokenStream {
        let modify_error = &self.modify_error;
        let column_enum = &self.column_enum;
        let entity = self.entity;

        // Nested variants: both Modify and Create for each child
        let nested_variants: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant =
                    syn::Ident::new(&format!("{}Modify", n.variant_name), Span::call_site());
                let create_variant =
                    syn::Ident::new(&format!("{}Create", n.variant_name), Span::call_site());
                let child_modify_ty = n.modify_error_ty();
                let child_create_ty = n.create_error_ty();
                vec![
                    quote! { #modify_variant(#child_modify_ty), },
                    quote! { #create_variant(#child_create_ty), },
                ]
            })
            .collect();
        let nested_display_arms: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant = syn::Ident::new(
                    &format!("{}Modify", n.variant_name),
                    Span::call_site(),
                );
                let create_variant = syn::Ident::new(
                    &format!("{}Create", n.variant_name),
                    Span::call_site(),
                );
                vec![
                    quote! { Self::#modify_variant(e) => write!(f, "{}: {}", stringify!(#modify_variant), e), },
                    quote! { Self::#create_variant(e) => write!(f, "{}: {}", stringify!(#create_variant), e), },
                ]
            })
            .collect();
        let nested_source_arms: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant =
                    syn::Ident::new(&format!("{}Modify", n.variant_name), Span::call_site());
                let create_variant =
                    syn::Ident::new(&format!("{}Create", n.variant_name), Span::call_site());
                vec![
                    quote! { Self::#modify_variant(e) => Some(e), },
                    quote! { Self::#create_variant(e) => Some(e), },
                ]
            })
            .collect();
        let nested_cm_checks: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant =
                    syn::Ident::new(&format!("{}Modify", n.variant_name), Span::call_site());
                let create_variant =
                    syn::Ident::new(&format!("{}Create", n.variant_name), Span::call_site());
                vec![
                    quote! { Self::#modify_variant(e) => e.was_concurrent_modification(), },
                    quote! { Self::#create_variant(e) => e.was_concurrent_modification(), },
                ]
            })
            .collect();

        let nested_dv_checks: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant =
                    syn::Ident::new(&format!("{}Modify", n.variant_name), Span::call_site());
                let create_variant =
                    syn::Ident::new(&format!("{}Create", n.variant_name), Span::call_site());
                vec![
                    quote! { Self::#modify_variant(e) => e.duplicate_value(), },
                    quote! { Self::#create_variant(e) => e.duplicate_value(), },
                ]
            })
            .collect();

        let nested_from_impls: Vec<_> = self
            .nested
            .iter()
            .flat_map(|n| {
                let modify_variant =
                    syn::Ident::new(&format!("{}Modify", n.variant_name), Span::call_site());
                let create_variant =
                    syn::Ident::new(&format!("{}Create", n.variant_name), Span::call_site());
                let child_modify_ty = n.modify_error_ty();
                let child_create_ty = n.create_error_ty();
                vec![
                    quote! {
                        impl From<#child_modify_ty> for #modify_error {
                            fn from(e: #child_modify_ty) -> Self {
                                Self::#modify_variant(e)
                            }
                        }
                    },
                    quote! {
                        impl From<#child_create_ty> for #modify_error {
                            fn from(e: #child_create_ty) -> Self {
                                Self::#create_variant(e)
                            }
                        }
                    },
                ]
            })
            .collect();

        let entity_name = entity.to_string();

        quote! {
            #[derive(Debug)]
            pub enum #modify_error {
                Sqlx(sqlx::Error),
                ConstraintViolation { column: Option<#column_enum>, value: Option<String>, inner: sqlx::Error },
                ConcurrentModification,
                PostPersistHookError(sqlx::Error),
                #(#nested_variants)*
            }

            impl std::fmt::Display for #modify_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}ModifyError - Sqlx: {}", #entity_name, e),
                        Self::ConstraintViolation { column, value, inner } => write!(f, "{}ModifyError - ConstraintViolation({:?}, {:?}): {}", #entity_name, column, value, inner),
                        Self::ConcurrentModification => write!(f, "{}ModifyError - ConcurrentModification", #entity_name),
                        Self::PostPersistHookError(e) => write!(f, "{}ModifyError - PostPersistHookError: {}", #entity_name, e),
                        #(#nested_display_arms)*
                    }
                }
            }

            impl std::error::Error for #modify_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::ConstraintViolation { inner, .. } => Some(inner),
                        Self::ConcurrentModification => None,
                        Self::PostPersistHookError(e) => Some(e),
                        #(#nested_source_arms)*
                    }
                }
            }

            impl From<sqlx::Error> for #modify_error {
                fn from(e: sqlx::Error) -> Self {
                    Self::Sqlx(e)
                }
            }

            #(#nested_from_impls)*

            impl #modify_error {
                pub fn was_concurrent_modification(&self) -> bool {
                    match self {
                        Self::ConcurrentModification => true,
                        #(#nested_cm_checks)*
                        _ => false,
                    }
                }

                pub fn was_duplicate(&self) -> bool {
                    matches!(self, Self::ConstraintViolation { .. })
                }

                pub fn was_duplicate_by(&self, column: #column_enum) -> bool {
                    matches!(self, Self::ConstraintViolation { column: Some(c), .. } if *c == column)
                }

                pub fn duplicate_value(&self) -> Option<&str> {
                    match self {
                        Self::ConstraintViolation { value: Some(v), .. } => Some(v.as_str()),
                        #(#nested_dv_checks)*
                        _ => None,
                    }
                }
            }
        }
    }

    fn generate_find_error(&self) -> TokenStream {
        let find_error = &self.find_error;
        let query_error = &self.query_error;
        let column_enum = &self.column_enum;
        let entity = self.entity;
        let entity_name = entity.to_string();

        let (ph_variant, ph_display_arm, ph_source_arm, ph_from_arm, ph_helper) = if let Some(
            config,
        ) =
            &self.post_hydrate_hook
        {
            let error_ty = &config.error;
            (
                quote! { PostHydrateError(#error_ty), },
                quote! { Self::PostHydrateError(e) => write!(f, "{}FindError - PostHydrateError: {}", #entity_name, e), },
                quote! { Self::PostHydrateError(e) => Some(e), },
                quote! { #query_error::PostHydrateError(e) => Self::PostHydrateError(e), },
                quote! {
                    pub fn was_post_hydrate_error(&self) -> bool {
                        matches!(self, Self::PostHydrateError(..))
                    }
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {}, quote! {}, quote! {})
        };

        quote! {
            #[derive(Debug)]
            pub enum #find_error {
                Sqlx(sqlx::Error),
                NotFound { entity: &'static str, column: Option<#column_enum>, value: String },
                HydrationError(es_entity::EntityHydrationError),
                #ph_variant
            }

            impl std::fmt::Display for #find_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}FindError - Sqlx: {}", #entity_name, e),
                        Self::NotFound { entity, column: Some(column), value } => write!(f, "{}FindError - NotFound({column}={value})", entity),
                        Self::NotFound { entity, column: None, value } => write!(f, "{}FindError - NotFound({})", entity, value),
                        Self::HydrationError(e) => write!(f, "{}FindError - HydrationError: {}", #entity_name, e),
                        #ph_display_arm
                    }
                }
            }

            impl std::error::Error for #find_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::NotFound { .. } => None,
                        Self::HydrationError(e) => Some(e),
                        #ph_source_arm
                    }
                }
            }

            impl From<sqlx::Error> for #find_error {
                fn from(e: sqlx::Error) -> Self {
                    Self::Sqlx(e)
                }
            }

            impl From<es_entity::EntityHydrationError> for #find_error {
                fn from(e: es_entity::EntityHydrationError) -> Self {
                    Self::HydrationError(e)
                }
            }

            impl From<#query_error> for #find_error {
                fn from(e: #query_error) -> Self {
                    match e {
                        #query_error::Sqlx(e) => Self::Sqlx(e),
                        #query_error::HydrationError(e) => Self::HydrationError(e),
                        #query_error::CursorDestructureError(_) => unreachable!("CursorDestructureError cannot occur in find operations"),
                        #ph_from_arm
                    }
                }
            }

            impl #find_error {
                pub fn was_not_found(&self) -> bool {
                    matches!(self, Self::NotFound { .. })
                }

                pub fn was_not_found_by(&self, column: #column_enum) -> bool {
                    matches!(self, Self::NotFound { column: Some(c), .. } if *c == column)
                }

                pub fn not_found_value(&self) -> Option<&str> {
                    match self {
                        Self::NotFound { value, .. } => Some(value.as_str()),
                        _ => None,
                    }
                }

                #ph_helper
            }
        }
    }

    fn generate_query_error(&self) -> TokenStream {
        let query_error = &self.query_error;
        let entity = self.entity;
        let entity_name = entity.to_string();

        let (ph_variant, ph_display_arm, ph_source_arm, ph_helper) = if let Some(config) =
            &self.post_hydrate_hook
        {
            let error_ty = &config.error;
            (
                quote! { PostHydrateError(#error_ty), },
                quote! { Self::PostHydrateError(e) => write!(f, "{}QueryError - PostHydrateError: {}", #entity_name, e), },
                quote! { Self::PostHydrateError(e) => Some(e), },
                quote! {
                    pub fn was_post_hydrate_error(&self) -> bool {
                        matches!(self, Self::PostHydrateError(..))
                    }
                },
            )
        } else {
            (quote! {}, quote! {}, quote! {}, quote! {})
        };

        quote! {
            #[derive(Debug)]
            pub enum #query_error {
                Sqlx(sqlx::Error),
                HydrationError(es_entity::EntityHydrationError),
                CursorDestructureError(es_entity::CursorDestructureError),
                #ph_variant
            }

            impl std::fmt::Display for #query_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}QueryError - Sqlx: {}", #entity_name, e),
                        Self::HydrationError(e) => write!(f, "{}QueryError - HydrationError: {}", #entity_name, e),
                        Self::CursorDestructureError(e) => write!(f, "{}QueryError - CursorDestructureError: {}", #entity_name, e),
                        #ph_display_arm
                    }
                }
            }

            impl std::error::Error for #query_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::HydrationError(e) => Some(e),
                        Self::CursorDestructureError(e) => Some(e),
                        #ph_source_arm
                    }
                }
            }

            impl From<sqlx::Error> for #query_error {
                fn from(e: sqlx::Error) -> Self {
                    Self::Sqlx(e)
                }
            }

            impl From<es_entity::EntityHydrationError> for #query_error {
                fn from(e: es_entity::EntityHydrationError) -> Self {
                    Self::HydrationError(e)
                }
            }

            impl From<es_entity::CursorDestructureError> for #query_error {
                fn from(e: es_entity::CursorDestructureError) -> Self {
                    Self::CursorDestructureError(e)
                }
            }

            impl #query_error {
                #ph_helper
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

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::{Ident, parse_quote};

    fn make_error_types(nested: Vec<NestedErrorInfo>) -> ErrorTypes<'static> {
        // Leak entity ident to get a 'static reference for tests
        let entity: &'static syn::Ident =
            Box::leak(Box::new(Ident::new("Order", Span::call_site())));
        let post_hydrate_hook: &'static Option<PostHydrateHookConfig> = Box::leak(Box::new(None));
        ErrorTypes {
            entity,
            column_enum: Ident::new("OrderColumn", Span::call_site()),
            create_error: Ident::new("OrderCreateError", Span::call_site()),
            modify_error: Ident::new("OrderModifyError", Span::call_site()),
            find_error: Ident::new("OrderFindError", Span::call_site()),
            query_error: Ident::new("OrderQueryError", Span::call_site()),
            column_variants: vec![],
            nested,
            post_hydrate_hook,
        }
    }

    #[test]
    fn non_generic_nested_uses_associated_type() {
        let error_types = make_error_types(vec![NestedErrorInfo {
            child_repo_ty: parse_quote! { ItemRepo },
            variant_name: Ident::new("Items", Span::call_site()),
            nested_entity: None,
        }]);

        let tokens = error_types.generate_create_error();
        let output = tokens.to_string();

        // Should use associated type projection (existing behavior)
        assert!(
            output.contains("< ItemRepo as es_entity :: EsRepo > :: CreateError"),
            "Expected associated type projection, got: {}",
            output
        );
    }

    #[test]
    fn generic_nested_with_entity_uses_concrete_name() {
        let error_types = make_error_types(vec![NestedErrorInfo {
            child_repo_ty: parse_quote! { ItemRepo<Evt> },
            variant_name: Ident::new("Items", Span::call_site()),
            nested_entity: Some(Ident::new("InterestAccrualCycle", Span::call_site())),
        }]);

        let tokens = error_types.generate_create_error();
        let output = tokens.to_string();

        // Should use concrete error type name, NOT associated type projection
        assert!(
            output.contains("InterestAccrualCycleCreateError"),
            "Expected concrete error type name, got: {}",
            output
        );
        assert!(
            !output.contains("ItemRepo"),
            "Should not reference the generic repo type, got: {}",
            output
        );
    }

    #[test]
    fn generic_nested_modify_error_uses_concrete_names() {
        let error_types = make_error_types(vec![NestedErrorInfo {
            child_repo_ty: parse_quote! { ItemRepo<Evt> },
            variant_name: Ident::new("Items", Span::call_site()),
            nested_entity: Some(Ident::new("InterestAccrualCycle", Span::call_site())),
        }]);

        let tokens = error_types.generate_modify_error();
        let output = tokens.to_string();

        // Should use concrete error type names for both Modify and Create variants
        assert!(
            output.contains("InterestAccrualCycleModifyError"),
            "Expected concrete modify error type, got: {}",
            output
        );
        assert!(
            output.contains("InterestAccrualCycleCreateError"),
            "Expected concrete create error type, got: {}",
            output
        );
        assert!(
            !output.contains("ItemRepo"),
            "Should not reference the generic repo type, got: {}",
            output
        );
    }

    #[test]
    fn mixed_nested_repos() {
        let error_types = make_error_types(vec![
            NestedErrorInfo {
                child_repo_ty: parse_quote! { ItemRepo },
                variant_name: Ident::new("Items", Span::call_site()),
                nested_entity: None,
            },
            NestedErrorInfo {
                child_repo_ty: parse_quote! { AccrualRepo<Evt> },
                variant_name: Ident::new("Accruals", Span::call_site()),
                nested_entity: Some(Ident::new("Accrual", Span::call_site())),
            },
        ]);

        let tokens = error_types.generate_create_error();
        let output = tokens.to_string();

        // Non-generic nested should use associated type projection
        assert!(
            output.contains("< ItemRepo as es_entity :: EsRepo > :: CreateError"),
            "Expected associated type projection for non-generic repo, got: {}",
            output
        );
        // Generic nested with entity should use concrete name
        assert!(
            output.contains("AccrualCreateError"),
            "Expected concrete error type for generic repo, got: {}",
            output
        );
    }

    #[test]
    fn auto_derive_entity_from_repo_type_name() {
        // When nested_entity is derived automatically (via derive_entity_from_repo_type),
        // it strips the "Repo" suffix: ObligationRepo<Evt> → Obligation
        let error_types = make_error_types(vec![NestedErrorInfo {
            child_repo_ty: parse_quote! { ObligationRepo<Evt> },
            variant_name: Ident::new("Obligations", Span::call_site()),
            nested_entity: derive_entity_from_repo_type(&parse_quote! { ObligationRepo<Evt> }),
        }]);

        let tokens = error_types.generate_create_error();
        let output = tokens.to_string();

        assert!(
            output.contains("ObligationCreateError"),
            "Expected auto-derived concrete error type, got: {}",
            output
        );
        assert!(
            !output.contains("ObligationRepo"),
            "Should not reference the generic repo type, got: {}",
            output
        );
    }

    #[test]
    fn type_uses_any_generic_detects_params() {
        let evt = Ident::new("Evt", Span::call_site());
        let idents = vec![&evt];

        // Type with generic param
        let ty: syn::Type = parse_quote! { SomeRepo<Evt> };
        assert!(type_uses_any_generic(&ty, &idents));

        // Type without generic param
        let ty: syn::Type = parse_quote! { SomeRepo };
        assert!(!type_uses_any_generic(&ty, &idents));

        // Type with different generic param
        let ty: syn::Type = parse_quote! { SomeRepo<Other> };
        assert!(!type_uses_any_generic(&ty, &idents));
    }

    #[test]
    fn derive_entity_strips_repo_suffix() {
        let ty: syn::Type = parse_quote! { ObligationRepo<Evt> };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "Obligation");

        let ty: syn::Type = parse_quote! { InterestAccrualRepo<E> };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "InterestAccrual");

        // Non-generic also works
        let ty: syn::Type = parse_quote! { ItemRepo };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "Item");
    }

    #[test]
    fn derive_entity_singularizes_plural_name() {
        // Plural → singular convention
        let ty: syn::Type = parse_quote! { OrderItems<Evt> };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "OrderItem");

        let ty: syn::Type = parse_quote! { BillingPeriods<E> };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "BillingPeriod");

        // Non-generic plural also works
        let ty: syn::Type = parse_quote! { Users };
        let entity = derive_entity_from_repo_type(&ty);
        assert_eq!(entity.unwrap().to_string(), "User");
    }

    #[test]
    fn derive_entity_returns_none_for_unrecognized() {
        // Neither Repo suffix nor plural → None
        let ty: syn::Type = parse_quote! { SomeType<E> };
        assert!(derive_entity_from_repo_type(&ty).is_none());

        // Singular name without Repo suffix → None
        let ty: syn::Type = parse_quote! { Obligation<E> };
        assert!(derive_entity_from_repo_type(&ty).is_none());
    }
}
