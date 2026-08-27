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

    pub fn find_nested_include_deleted_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("find_nested_{}_include_deleted_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn delete_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("delete_nested_{}_in_op", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn pending_nested_work_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("has_pending_nested_{}_work", self.ident()),
            proc_macro2::Span::call_site(),
        )
    }

    pub fn forget_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("forget_nested_{}_in_op", self.ident()),
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
    #[darling(default)]
    forgettable: bool,
    #[darling(default, rename = "forgettable_tbl")]
    forgettable_table_name: Option<String>,

    /// Override the migrations directory the index catalog is derived from.
    /// Resolved relative to `$CARGO_MANIFEST_DIR`. Takes effect only when the
    /// `ES_ENTITY_MIGRATIONS_DIR` environment variable (set by a `build.rs`
    /// recipe, or directly) is not present. Defaults to
    /// `$CARGO_MANIFEST_DIR/migrations` when that directory exists.
    #[darling(default)]
    migrations_dir: Option<String>,
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

        if self.forgettable && self.forgettable_table_name.is_none() {
            self.forgettable_table_name = Some(format!(
                "{}_forgettable_payloads",
                self.table_name.as_ref().expect("Table name not set")
            ));
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

    /// Resolve the migrations directory this repo derives its index catalog and
    /// error-constraint names from. Resolution order:
    ///   1. `ES_ENTITY_MIGRATIONS_DIR` env var (a `build.rs` `rustc-env`, or a
    ///      plain env var — both visible via `std::env::var` at expansion),
    ///   2. the `#[es_repo(migrations_dir = "…")]` attribute (relative to
    ///      `$CARGO_MANIFEST_DIR`),
    ///   3. auto-discovery: `$CARGO_MANIFEST_DIR/migrations`, then a `migrations`
    ///      directory in any ancestor up to the repository root (the first
    ///      directory containing `.git`) — so a single-crate app or a workspace
    ///      with root-level `migrations/` works with zero configuration.
    ///
    /// Returns `None` when nothing resolves (e.g. migrations in a sibling
    /// subtree of another crate) — the caller then uses an empty catalog and
    /// everything still works, just without index-driven specialization.
    fn migrations_dir(&self) -> Option<std::path::PathBuf> {
        use std::path::{Path, PathBuf};

        if let Ok(dir) = std::env::var("ES_ENTITY_MIGRATIONS_DIR") {
            let dir = PathBuf::from(dir);
            return dir.is_dir().then_some(dir);
        }

        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
        let manifest_dir = Path::new(&manifest_dir);

        if let Some(rel) = &self.migrations_dir {
            let dir = manifest_dir.join(rel);
            return dir.is_dir().then_some(dir);
        }

        // Auto-discovery: crate-local `migrations/`, then walk up to the repo
        // root (a directory containing `.git`), checking each ancestor.
        let mut cur = Some(manifest_dir);
        while let Some(dir) = cur {
            let candidate = dir.join("migrations");
            if candidate.is_dir() {
                return Some(candidate);
            }
            if dir.join(".git").exists() {
                break; // reached the repo root; do not escape it
            }
            cur = dir.parent();
        }
        None
    }

    /// Derive the physical index catalog for this repo's table from the resolved
    /// migrations directory (see [`Self::migrations_dir`]). An empty catalog
    /// (no directory resolved) is safe: `list_for_filters` combinations fall
    /// back to the correct non-sargable `COALESCE` query, and error mapping
    /// keeps its attribute-derived name convention.
    pub fn index_catalog(&self) -> crate::index_catalog::IndexCatalog {
        match self.migrations_dir() {
            Some(dir) => {
                crate::index_catalog::IndexCatalog::from_migrations_dir(&dir).unwrap_or_default()
            }
            None => crate::index_catalog::IndexCatalog::default(),
        }
    }

    /// The resolved migration `.sql` files (sorted by name), or empty when no
    /// migrations directory is discoverable.
    fn migration_files(&self) -> Vec<std::path::PathBuf> {
        let Some(dir) = self.migrations_dir() else {
            return Vec::new();
        };
        let Ok(read) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files: Vec<std::path::PathBuf> = read
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sql"))
            .collect();
        files.sort();
        files
    }

    /// `include_bytes!` of every resolved migration file, so Cargo re-runs the
    /// derive — rebuilding the index catalog (and the error-constraint mapping)
    /// — whenever a migration is *edited*. This is what makes the zero-config
    /// migrations auto-discovery (including an ancestor `migrations/` that no
    /// crate-local `build.rs` watches) actually stay in sync.
    ///
    /// Skipped when `ES_ENTITY_MIGRATIONS_DIR` is set: that variable is emitted
    /// by a `build.rs` recipe which also emits `rerun-if-changed` on the
    /// directory (covering edits *and* additions), making per-file dependencies
    /// redundant. Without that recipe, picking up a newly *added* migration
    /// still needs a rebuild trigger (`cargo clean`, or the `build.rs`).
    pub fn migrations_rerun_tokens(&self) -> proc_macro2::TokenStream {
        if std::env::var_os("ES_ENTITY_MIGRATIONS_DIR").is_some() {
            return quote! {};
        }
        let includes = self.migration_files().into_iter().filter_map(|path| {
            let path = path.to_str()?;
            Some(quote! { const _: &[u8] = include_bytes!(#path); })
        });
        quote! { #(#includes)* }
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

    /// Whether this repo is an *aggregate root*: it owns nested children but is
    /// not itself a child of anything.
    ///
    /// Root-ness is inferred, never declared. A repo is a root iff it has
    /// `#[es_repo(nested)]` fields **and** no column marked `parent`. This
    /// deliberately excludes mid-level repos in grandchildren trees, which have
    /// both (children below, a `parent` column above): they are parents but not
    /// roots, and carry no version machinery.
    ///
    /// The root's index-table `version` column is the clock for the whole
    /// aggregate — see `book/src/aggregate-version.md`.
    pub fn is_root(&self) -> bool {
        self.any_nested() && self.columns.parent().is_none()
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

    pub fn forget_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}ForgetError", self.entity_ident),
            Span::call_site(),
        )
    }

    pub fn column_enum(&self) -> syn::Ident {
        syn::Ident::new(&format!("{}Column", self.entity_ident), Span::call_site())
    }

    /// The generated scope enum ident (`{Entity}Scope`), entity-named like
    /// the other generated companion types (`{Entity}FindError`,
    /// `{Entity}ByIdCursor`, ...).
    pub fn scope_type_ident(&self) -> syn::Ident {
        syn::Ident::new(&format!("{}Scope", self.entity_ident), Span::call_site())
    }

    /// The generated bound-view ident (`Scoped{Repo}`), repo-named because it
    /// is a view of the repository itself (returned by `repo.scoped(scope)`).
    pub fn scoped_view_ident(&self) -> syn::Ident {
        syn::Ident::new(&format!("Scoped{}", self.ident), Span::call_site())
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

    pub fn forgettable_enabled(&self) -> bool {
        self.forgettable
    }

    /// Errors if the repo declares `Forgettable<T>` index columns but does not
    /// enable `forgettable`. Both facts are known at macro time (unlike event
    /// forgettable-ness, which the repo cannot see — that is guarded by a
    /// const assert in the generated code instead).
    pub fn validate_forgettable(&self) -> darling::Result<()> {
        if !self.forgettable && !self.columns.forgettable_column_names().is_empty() {
            return Err(darling::Error::custom(
                "repo has Forgettable<T> index columns but does not enable `forgettable`; \
                 add `forgettable` to #[es_repo(...)]",
            ));
        }
        Ok(())
    }

    pub fn forgettable_table_name(&self) -> Option<&str> {
        if self.forgettable {
            Some(self.forgettable_table_name.as_deref().unwrap_or_else(|| {
                // Lazy init not possible with &str, so we use a different approach
                panic!("forgettable_table_name should have been set in update_defaults")
            }))
        } else {
            None
        }
    }
}
