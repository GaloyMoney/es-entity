use convert_case::{Case, Casing};
use proc_macro2::{Span, TokenStream};
use quote::quote;

use super::options::RepositoryOptions;

pub struct ErrorTypes<'a> {
    entity: &'a syn::Ident,
    column_enum: syn::Ident,
    create_error: syn::Ident,
    modify_error: syn::Ident,
    find_error: syn::Ident,
    query_error: syn::Ident,
    column_variants: Vec<ColumnVariant>,
    nested: Vec<NestedErrorInfo>,
}

struct ColumnVariant {
    variant_name: syn::Ident,
    column_name: String,
    constraint_names: Vec<String>,
}

struct NestedErrorInfo {
    child_repo_ty: syn::Type,
    variant_name: syn::Ident,
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

        let nested: Vec<NestedErrorInfo> = opts
            .all_nested()
            .map(|f| NestedErrorInfo {
                child_repo_ty: f.ty.clone(),
                variant_name: f.nested_variant_name(),
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
                let child_repo_ty = &n.child_repo_ty;
                quote! { #variant(<#child_repo_ty as es_entity::EsRepo>::CreateError), }
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
                let child_repo_ty = &n.child_repo_ty;
                quote! {
                    impl From<<#child_repo_ty as es_entity::EsRepo>::CreateError> for #create_error {
                        fn from(e: <#child_repo_ty as es_entity::EsRepo>::CreateError) -> Self {
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

        let entity_name = entity.to_string();

        quote! {
            #[derive(Debug)]
            pub enum #create_error {
                Sqlx(sqlx::Error),
                ConstraintViolation { column: Option<#column_enum>, value: Option<String>, inner: sqlx::Error },
                ConcurrentModification,
                HydrationError(es_entity::EntityHydrationError),
                PostPersistHookError(sqlx::Error),
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
                        #(#nested_source_arms)*
                    }
                }
            }

            impl From<sqlx::Error> for #create_error {
                fn from(e: sqlx::Error) -> Self {
                    Self::Sqlx(e)
                }
            }

            impl es_entity::FromConcurrentModification for #create_error {
                fn concurrent_modification() -> Self {
                    Self::ConcurrentModification
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

                pub fn was_duplicate(&self, column: #column_enum) -> bool {
                    matches!(self, Self::ConstraintViolation { column: Some(c), .. } if *c == column)
                }

                pub fn duplicate_value(&self) -> Option<&str> {
                    match self {
                        Self::ConstraintViolation { value: Some(v), .. } => Some(v.as_str()),
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
                let child_repo_ty = &n.child_repo_ty;
                vec![
                    quote! { #modify_variant(<#child_repo_ty as es_entity::EsRepo>::ModifyError), },
                    quote! { #create_variant(<#child_repo_ty as es_entity::EsRepo>::CreateError), },
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

        let nested_from_impls: Vec<_> = self
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
                let child_repo_ty = &n.child_repo_ty;
                vec![
                    quote! {
                        impl From<<#child_repo_ty as es_entity::EsRepo>::ModifyError> for #modify_error {
                            fn from(e: <#child_repo_ty as es_entity::EsRepo>::ModifyError) -> Self {
                                Self::#modify_variant(e)
                            }
                        }
                    },
                    quote! {
                        impl From<<#child_repo_ty as es_entity::EsRepo>::CreateError> for #modify_error {
                            fn from(e: <#child_repo_ty as es_entity::EsRepo>::CreateError) -> Self {
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

            impl es_entity::FromConcurrentModification for #modify_error {
                fn concurrent_modification() -> Self {
                    Self::ConcurrentModification
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

                pub fn was_duplicate(&self, column: #column_enum) -> bool {
                    matches!(self, Self::ConstraintViolation { column: Some(c), .. } if *c == column)
                }

                pub fn duplicate_value(&self) -> Option<&str> {
                    match self {
                        Self::ConstraintViolation { value: Some(v), .. } => Some(v.as_str()),
                        _ => None,
                    }
                }
            }
        }
    }

    fn generate_find_error(&self) -> TokenStream {
        let find_error = &self.find_error;
        let query_error = &self.query_error;
        let entity = self.entity;
        let entity_name = entity.to_string();

        quote! {
            #[derive(Debug)]
            pub enum #find_error {
                Sqlx(sqlx::Error),
                NotFound { entity: &'static str, column: &'static str, value: String },
                HydrationError(es_entity::EntityHydrationError),
            }

            impl std::fmt::Display for #find_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}FindError - Sqlx: {}", #entity_name, e),
                        Self::NotFound { entity, column, value } => write!(f, "{}FindError - NotFound({column}={value})", entity),
                        Self::HydrationError(e) => write!(f, "{}FindError - HydrationError: {}", #entity_name, e),
                    }
                }
            }

            impl std::error::Error for #find_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::NotFound { .. } => None,
                        Self::HydrationError(e) => Some(e),
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
                    }
                }
            }

            impl #find_error {
                pub fn was_not_found(&self) -> bool {
                    matches!(self, Self::NotFound { .. })
                }
            }
        }
    }

    fn generate_query_error(&self) -> TokenStream {
        let query_error = &self.query_error;
        let entity = self.entity;
        let entity_name = entity.to_string();

        quote! {
            #[derive(Debug)]
            pub enum #query_error {
                Sqlx(sqlx::Error),
                HydrationError(es_entity::EntityHydrationError),
                CursorDestructureError(es_entity::CursorDestructureError),
            }

            impl std::fmt::Display for #query_error {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match self {
                        Self::Sqlx(e) => write!(f, "{}QueryError - Sqlx: {}", #entity_name, e),
                        Self::HydrationError(e) => write!(f, "{}QueryError - HydrationError: {}", #entity_name, e),
                        Self::CursorDestructureError(e) => write!(f, "{}QueryError - CursorDestructureError: {}", #entity_name, e),
                    }
                }
            }

            impl std::error::Error for #query_error {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    match self {
                        Self::Sqlx(e) => Some(e),
                        Self::HydrationError(e) => Some(e),
                        Self::CursorDestructureError(e) => Some(e),
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
        }
    }
}
