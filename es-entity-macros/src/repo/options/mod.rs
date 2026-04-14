mod columns;
mod delete;

use convert_case::{Case, Casing};
use darling::{FromDeriveInput, FromField, FromMeta};
use proc_macro2::Span;
use quote::quote;

pub use columns::*;
pub use delete::*;

#[derive(Debug, Clone)]
pub struct PostPersistHookConfig {
    pub method: syn::Ident,
    pub error: syn::Type,
}

impl FromMeta for PostPersistHookConfig {
    /// Old syntax: `post_persist_hook = "method_name"` → defaults error to `sqlx::Error`
    fn from_string(value: &str) -> darling::Result<Self> {
        Ok(PostPersistHookConfig {
            method: syn::Ident::new(value, Span::call_site()),
            error: syn::parse_str("sqlx::Error")
                .map_err(|e| darling::Error::custom(format!("invalid error type: {e}")))?,
        })
    }

    /// New syntax: `post_persist_hook(method = "...", error = "...")`
    /// `error` defaults to `sqlx::Error` if omitted
    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let mut method: Option<syn::Ident> = None;
        let mut error: Option<syn::Type> = None;

        for item in items {
            if let darling::ast::NestedMeta::Meta(syn::Meta::NameValue(nv)) = item {
                if nv.path.is_ident("method")
                    && let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                {
                    method = Some(syn::Ident::new(&s.value(), s.span()));
                } else if nv.path.is_ident("error")
                    && let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                {
                    error =
                        Some(syn::parse_str(&s.value()).map_err(|e| {
                            darling::Error::custom(format!("invalid error type: {e}"))
                        })?);
                }
            }
        }

        let error = error
            .unwrap_or_else(|| syn::parse_str("sqlx::Error").expect("sqlx::Error is a valid type"));

        Ok(PostPersistHookConfig {
            method: method
                .ok_or_else(|| darling::Error::custom("missing `method` in post_persist_hook"))?,
            error,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PostHydrateHookConfig {
    pub method: syn::Ident,
    pub error: syn::Type,
}

impl FromMeta for PostHydrateHookConfig {
    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let mut method: Option<syn::Ident> = None;
        let mut error: Option<syn::Type> = None;

        for item in items {
            if let darling::ast::NestedMeta::Meta(syn::Meta::NameValue(nv)) = item {
                if nv.path.is_ident("method") {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                    {
                        method = Some(syn::Ident::new(&s.value(), s.span()));
                    }
                } else if nv.path.is_ident("error")
                    && let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                {
                    error =
                        Some(syn::parse_str(&s.value()).map_err(|e| {
                            darling::Error::custom(format!("invalid error type: {e}"))
                        })?);
                }
            }
        }

        Ok(PostHydrateHookConfig {
            method: method
                .ok_or_else(|| darling::Error::custom("missing `method` in post_hydrate_hook"))?,
            error: error
                .ok_or_else(|| darling::Error::custom("missing `error` in post_hydrate_hook"))?,
        })
    }
}

/// Information about the clock field in a repository
#[derive(Debug, Clone)]
pub enum ClockFieldInfo<'a> {
    /// No clock field present
    None,
    /// Clock field is `Option<ClockHandle>` - use if Some, fallback to global
    Optional(&'a syn::Ident),
    /// Clock field is `ClockHandle` - always use it
    Required(&'a syn::Ident),
}

#[derive(FromField)]
#[darling(attributes(es_repo))]
pub struct RepoField {
    pub ident: Option<syn::Ident>,
    pub ty: syn::Type,
    #[darling(default)]
    pub pool: bool,
    #[darling(default)]
    pub clock: bool,
    #[darling(default)]
    pub nested: bool,
    /// For nested fields whose repo type is generic, specify the child entity name
    /// so error types can be referenced concretely (e.g., `entity = "InterestAccrualCycle"`
    /// generates `InterestAccrualCycleCreateError` instead of
    /// `<InterestAccrualRepo<Evt> as EsRepo>::CreateError`).
    #[darling(default)]
    pub entity: Option<syn::Ident>,
}

impl RepoField {
    pub fn ident(&self) -> &syn::Ident {
        self.ident.as_ref().expect("Field must have an identifier")
    }

    fn is_pool_field(&self) -> bool {
        self.pool || self.ident.as_ref().is_some_and(|i| i == "pool")
    }

    fn is_clock_field(&self) -> bool {
        self.clock || self.ident.as_ref().is_some_and(|i| i == "clock")
    }

    /// Check if the field type is `Option<...>`
    fn is_option_type(&self) -> bool {
        if let syn::Type::Path(type_path) = &self.ty
            && let Some(segment) = type_path.path.segments.last()
        {
            return segment.ident == "Option";
        }
        false
    }

    pub fn create_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("create_nested_{}_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn update_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("update_nested_{}_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn find_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("find_nested_{}_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn delete_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("delete_nested_{}_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    /// PascalCase variant name derived from field name (e.g. `line_items` -> `LineItems`)
    pub fn nested_variant_name(&self) -> syn::Ident {
        syn::Ident::new(
            &self.ident().to_string().to_case(Case::UpperCamel),
            Span::call_site(),
        )
    }
}

#[derive(FromDeriveInput)]
#[darling(attributes(es_repo), map = "Self::update_defaults")]
pub struct RepositoryOptions {
    pub ident: syn::Ident,
    pub generics: syn::Generics,
    #[darling(default)]
    pub columns: Columns,
    #[darling(default)]
    pub post_persist_hook: Option<PostPersistHookConfig>,
    #[darling(default)]
    pub post_hydrate_hook: Option<PostHydrateHookConfig>,
    #[darling(default)]
    pub delete: DeleteOption,

    data: darling::ast::Data<(), RepoField>,

    #[darling(rename = "entity")]
    entity_ident: syn::Ident,
    #[darling(default, rename = "event")]
    event_ident: Option<syn::Ident>,
    #[darling(default, rename = "id")]
    id_ty: Option<syn::Ident>,
    #[darling(default, rename = "tbl_prefix")]
    prefix: Option<syn::LitStr>,
    #[darling(default, rename = "tbl")]
    table_name: Option<String>,
    #[darling(default, rename = "events_tbl")]
    events_table_name: Option<String>,

    #[darling(default)]
    persist_event_context: Option<bool>,
}

impl RepositoryOptions {
    fn update_defaults(mut self) -> Self {
        let entity_name = self.entity_ident.to_string();
        if self.event_ident.is_none() {
            self.event_ident = Some(syn::Ident::new(
                &format!("{entity_name}Event"),
                proc_macro2::Span::call_site(),
            ));
        }
        if self.id_ty.is_none() {
            self.id_ty = Some(syn::Ident::new(
                &format!("{entity_name}Id"),
                proc_macro2::Span::call_site(),
            ));
        }
        let prefix = if let Some(prefix) = &self.prefix {
            format!("{}_", prefix.value())
        } else {
            String::new()
        };
        if self.table_name.is_none() {
            self.table_name = Some(format!(
                "{prefix}{}",
                pluralizer::pluralize(&entity_name, 2, false).to_case(Case::Snake)
            ));
        }
        if self.events_table_name.is_none() {
            self.events_table_name =
                Some(format!("{prefix}{entity_name}Events").to_case(Case::Snake));
        }

        self.columns
            .set_id_column(self.id_ty.as_ref().expect("Id not set"));

        self
    }

    pub fn entity(&self) -> &syn::Ident {
        &self.entity_ident
    }

    pub fn table_name(&self) -> &str {
        self.table_name.as_ref().expect("Table name is not set")
    }

    pub fn table_prefix(&self) -> Option<&syn::LitStr> {
        self.prefix.as_ref()
    }

    pub fn id(&self) -> &syn::Ident {
        self.id_ty.as_ref().expect("ID identifier is not set")
    }

    pub fn event(&self) -> &syn::Ident {
        self.event_ident
            .as_ref()
            .expect("Event identifier is not set")
    }

    pub fn event_context_enabled(&self) -> bool {
        #[cfg(feature = "event-context-enabled")]
        {
            self.persist_event_context.unwrap_or(true)
        }
        #[cfg(not(feature = "event-context-enabled"))]
        {
            self.persist_event_context.unwrap_or(false)
        }
    }

    pub fn events_table_name(&self) -> &str {
        self.events_table_name
            .as_ref()
            .expect("Events table name is not set")
    }

    pub fn cursor_mod(&self) -> syn::Ident {
        let name = format!("{}Cursor", self.entity_ident).to_case(Case::Snake);
        syn::Ident::new(&name, proc_macro2::Span::call_site())
    }

    pub fn repo_types_mod(&self) -> syn::Ident {
        let name = format!("{}RepoTypes", self.entity_ident).to_case(Case::Snake);
        syn::Ident::new(&name, proc_macro2::Span::call_site())
    }

    #[cfg(feature = "instrument")]
    pub fn repo_name_snake_case(&self) -> String {
        self.ident.to_string().to_case(Case::Snake)
    }

    pub fn pool_field(&self) -> &syn::Ident {
        let field = match &self.data {
            darling::ast::Data::Struct(fields) => fields.iter().find_map(|field| {
                if field.is_pool_field() {
                    Some(field.ident.as_ref().unwrap())
                } else {
                    None
                }
            }),
            _ => None,
        };
        field.expect("Repo must have a field named 'pool' or marked with #[es_repo(pool)]")
    }

    pub fn clock_field(&self) -> ClockFieldInfo<'_> {
        match &self.data {
            darling::ast::Data::Struct(fields) => {
                for field in fields.iter() {
                    if field.is_clock_field() {
                        let ident = field.ident.as_ref().unwrap();
                        return if field.is_option_type() {
                            ClockFieldInfo::Optional(ident)
                        } else {
                            ClockFieldInfo::Required(ident)
                        };
                    }
                }
                ClockFieldInfo::None
            }
            _ => ClockFieldInfo::None,
        }
    }

    pub fn any_nested(&self) -> bool {
        if let darling::ast::Data::Struct(fields) = &self.data {
            fields.iter().any(|f| f.nested)
        } else {
            panic!("Repository must be a struct")
        }
    }

    pub fn all_nested(&self) -> impl Iterator<Item = &RepoField> {
        if let darling::ast::Data::Struct(fields) = &self.data {
            fields.iter().filter(|f| f.nested)
        } else {
            panic!("Repository must be a struct")
        }
    }

    pub fn query_fn_generics(nested: bool) -> proc_macro2::TokenStream {
        if nested {
            quote! {
                <OP>
            }
        } else {
            quote! {
                <'a, OP>
            }
        }
    }

    pub fn query_fn_op_arg(nested: bool) -> proc_macro2::TokenStream {
        if nested {
            quote! {
                op: &mut OP
            }
        } else {
            quote! {
                op: OP
            }
        }
    }

    pub fn query_fn_op_traits(nested: bool) -> proc_macro2::TokenStream {
        if nested {
            quote! {
                es_entity::AtomicOperation
            }
        } else {
            quote! {
                es_entity::IntoOneTimeExecutor<'a>
            }
        }
    }

    pub fn create_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}CreateError", self.entity_ident),
            Span::call_site(),
        )
    }

    pub fn modify_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}ModifyError", self.entity_ident),
            Span::call_site(),
        )
    }

    pub fn find_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}FindError", self.entity_ident),
            Span::call_site(),
        )
    }

    pub fn query_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}QueryError", self.entity_ident),
            Span::call_site(),
        )
    }

    pub fn column_enum(&self) -> syn::Ident {
        syn::Ident::new(&format!("{}Column", self.entity_ident), Span::call_site())
    }

    pub fn query_fn_get_op(nested: bool) -> proc_macro2::TokenStream {
        if nested {
            quote! {
                &mut self.pool().begin().await?
            }
        } else {
            quote! {
                self.pool()
            }
        }
    }
}
