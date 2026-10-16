mod columns;
mod delete;
mod id;

use convert_case::{Case, Casing};
use darling::{FromDeriveInput, FromField};
use proc_macro2::Span;
use quote::quote;

pub use columns::*;
pub use delete::*;
pub use id::*;

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
    /// so the constraint-violation type can be referenced concretely (e.g.,
    /// `entity = "InterestAccrualCycle"` generates `InterestAccrualCycleConstraintViolation`
    /// instead of `<InterestAccrualRepo<Evt> as EsRepo>::ConstraintViolation`).
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

    pub fn hydrate_nested_fn_name(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("hydrate_nested_{}", self.ident()),
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
    pub post_persist_hook: Option<syn::Ident>,
    #[darling(default)]
    pub post_hydrate_hook: Option<syn::Ident>,
    #[darling(default)]
    pub delete: DeleteOption,

    data: darling::ast::Data<(), RepoField>,

    #[darling(rename = "entity")]
    entity_ident: syn::Ident,
    #[darling(default, rename = "event")]
    event_ident: Option<syn::Ident>,
    #[darling(default, rename = "id")]
    id_opts: IdOpts,
    #[darling(skip)]
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
    in_op_only: bool,
    #[darling(default)]
    forgettable: bool,
    #[darling(default, rename = "forgettable_tbl")]
    forgettable_table_name: Option<String>,
    #[darling(default)]
    snapshot: bool,
    #[darling(default, rename = "snapshot_tbl")]
    snapshot_table_name: Option<String>,

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
        self.id_ty = Some(self.id_opts.ty.clone().unwrap_or_else(|| {
            syn::Ident::new(&format!("{entity_name}Id"), proc_macro2::Span::call_site())
        }));
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

        if self.snapshot && self.snapshot_table_name.is_none() {
            // Mirrors `events_table_name`'s convention (singular entity name),
            // not `forgettable_table_name`'s (pluralized `table_name`) —
            // every hand-written override anyone has reached for lands on
            // the singular form, matching the sibling `_events` table.
            self.snapshot_table_name =
                Some(format!("{prefix}{entity_name}Snapshots").to_case(Case::Snake));
        }

        self.columns
            .set_id_column(self.id_ty.as_ref().expect("Id not set"));
        if let Some(scope) = self.id_opts.scope.take() {
            self.columns.scope_id_column(scope);
        }

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

    /// Where this repo gets the id of an entity it creates.
    pub fn id_source(&self) -> IdSourceOpt {
        self.id_opts.source.clone()
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

    pub fn pool_field(&self) -> Option<&syn::Ident> {
        match &self.data {
            darling::ast::Data::Struct(fields) => fields.iter().find_map(|field| {
                if field.is_pool_field() {
                    Some(field.ident.as_ref().unwrap())
                } else {
                    None
                }
            }),
            _ => None,
        }
    }

    pub fn in_op_only(&self) -> bool {
        self.in_op_only
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

    /// Whether the generated `update`/`update_all` can ever reject: `true`
    /// unless both of the following hold, in which case the generated
    /// signatures narrow to `es_entity::RepoFault` instead of
    /// `RepoWriteError<CV>` — there is nothing left in the `Rejected` lane
    /// for them to ever construct.
    ///
    /// 1. No unique, primary-key or check constraint on the entity's table has
    ///    a column that persists on update. Foreign keys do not count: a
    ///    violation is an invariant fault, never a rejection, on the
    ///    precondition that referenced rows are not hard-deleted. Reusing [`crate::repo::error_types::enumerate_constraints`],
    ///    the same enumeration `ErrorTypes` builds the violation enum from,
    ///    so this sees exactly the same constraint set (id-only pkey
    ///    resolution via the catalog included). When the catalog has no
    ///    entry at all for the table, every constraint here is synthesized
    ///    from convention rather than read from a real migration, so which
    ///    persisted columns are genuinely constrained can't be known —
    ///    conservatively, this degrades to [`Columns::updates_needed`].
    ///
    /// This is the repo's *own* statement only. A nested child's writes run
    /// inside the parent's, and whether they can reject is the child's own
    /// answer, published as `es_entity::WriteShapes` on its violation enum
    /// and folded into the signature by [`Self::update_error_ty`].
    pub fn update_can_reject(&self) -> bool {
        let table = self.table_name();
        if self.index_catalog().table_constraints(table).is_empty() {
            return self.columns.updates_needed();
        }
        let updatable = |name: &String| {
            self.columns
                .column_enum_columns()
                .find(|c| c.name() == name)
                .is_some_and(|c| c.persist_on_update() && !c.is_id())
        };
        crate::repo::error_types::enumerate_constraints(self)
            .iter()
            .filter(|(_, kind, _)| {
                !matches!(kind, crate::index_catalog::ConstraintKind::ForeignKey)
            })
            .any(|(_, _, col_names)| col_names.iter().any(updatable))
    }

    /// Whether the repo's own create statement can ever reject: `true` unless
    /// every constraint in the family is one a create cannot hit. Foreign
    /// keys never count (a violation is a fault), and neither does the
    /// primary key when the repo generates its own ids, since a collision of a
    /// fresh id is a fault. Nested children are folded in by
    /// [`Self::create_error_ty`].
    pub fn create_can_reject(&self) -> bool {
        let generated = self.id_source().is_repo();
        crate::repo::error_types::enumerate_constraints(self)
            .iter()
            .filter(|(_, kind, _)| {
                !matches!(kind, crate::index_catalog::ConstraintKind::ForeignKey)
            })
            .any(|(name, kind, col_names)| {
                let unreachable_pkey =
                    generated && crate::repo::error_types::is_id_pkey(self, name, kind, col_names);
                !unreachable_pkey
            })
    }

    /// The disjuncts of the `create_or_find_in_op` lookup, id first.
    ///
    /// Empty on a scoped repo (`find_by_id_in_op` of a scoped repo needs a
    /// scope the create has no way to name) and on a soft-deleting one (a
    /// soft-deleted row still holds its key but is invisible to the lookup),
    /// and when the id is not a column the create writes as-is.
    ///
    /// Besides the id, a key is a unique constraint or non-partial unique
    /// index that the migrations declare for the table, on columns the create
    /// writes as-is. Without a catalog entry for the table nothing but the id
    /// is known to be unique.
    pub fn ensure_keys(&self) -> Vec<EnsureKey> {
        if !self.columns.scope_columns().is_empty() || self.delete.is_soft() {
            return Vec::new();
        }
        let table = self.table_name();
        let catalog = self.index_catalog();
        let catalog_table = table.to_lowercase();
        let constraints = crate::repo::error_types::enumerate_constraints(self);
        let columns: Vec<_> = self.columns.column_enum_columns().collect();
        let Some(id) = columns
            .iter()
            .find(|c| c.is_id())
            .filter(|c| c.is_ensurable())
        else {
            return Vec::new();
        };
        let mut keys = vec![EnsureKey::of(&[*id])];
        for (name, kind, col_names) in &constraints {
            if !matches!(kind, crate::index_catalog::ConstraintKind::Unique) {
                continue;
            }
            let declared = catalog.constraints.iter().any(|c| {
                c.table == catalog_table
                    && c.name == *name
                    && matches!(c.kind, crate::index_catalog::ConstraintKind::Unique)
            });
            let partial = catalog
                .entries
                .iter()
                .any(|e| e.table == catalog_table && e.partial && e.name.as_deref() == Some(name));
            if !declared || partial || col_names.is_empty() {
                continue;
            }
            let key_columns: Option<Vec<&Column>> = col_names
                .iter()
                .map(|n| columns.iter().find(|c| c.name() == n).copied())
                .collect();
            let Some(key_columns) = key_columns else {
                continue;
            };
            if !key_columns.iter().all(|c| c.is_ensurable()) {
                continue;
            }
            let key = EnsureKey::of(&key_columns);
            if keys.iter().any(|k| k.name == key.name) {
                continue;
            }
            keys.push(key);
        }
        keys
    }

    /// Whether `create_or_find_in_op` can reject: more than one disjunct can
    /// match more than one stored entity, a check constraint can reject the
    /// insert, and a nested child's create can reject.
    pub fn ensure_can_reject(&self) -> bool {
        self.any_nested()
            || self.ensure_keys().len() > 1
            || crate::repo::error_types::enumerate_constraints(self)
                .iter()
                .any(|(_, kind, _)| matches!(kind, crate::index_catalog::ConstraintKind::Check))
    }

    /// `{Entity}EnsureRejection`, the rejection of `create_or_find_in_op`.
    pub fn ensure_rejection(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}EnsureRejection", self.entity_ident),
            Span::call_site(),
        )
    }

    fn own_shape(can_reject: bool) -> proc_macro2::TokenStream {
        if can_reject {
            quote! { es_entity::Rejects }
        } else {
            quote! { es_entity::Faults }
        }
    }

    /// The `es_entity::WriteShape` of this repo's `create`/`create_all`: its
    /// own statement, then every nested child's create.
    pub fn create_shape(&self) -> proc_macro2::TokenStream {
        self.all_nested().fold(
            Self::own_shape(self.create_can_reject()),
            |shape, nested| {
                let child = Self::child_shapes(nested);
                quote! {
                    <#shape as es_entity::WriteShape>::Or<<#child>::Create>
                }
            },
        )
    }

    /// `<ChildConstraintViolation as es_entity::WriteShapes>`: where a nested
    /// child publishes its shapes. The child's violation enum is named rather
    /// than the child repo because it is always `pub`: a public parent whose
    /// signatures mention a less visible child repo would not compile.
    pub(crate) fn child_shapes(nested: &RepoField) -> proc_macro2::TokenStream {
        match crate::repo::error_types::nested_constraint_violation_path(nested) {
            Ok(path) => quote! { #path as es_entity::WriteShapes },
            // `ErrorTypes` reports the real error; keep this one quiet.
            Err(_) => quote! { () as es_entity::WriteShapes },
        }
    }

    /// The `es_entity::WriteShape` of this repo's `update`/`update_all` (and,
    /// with the same gate, `forget`/`delete`): its own statement, then every
    /// nested child's update and the create of its new children.
    pub fn update_shape(&self) -> proc_macro2::TokenStream {
        self.all_nested().fold(
            Self::own_shape(self.update_can_reject()),
            |shape, nested| {
                let child = Self::child_shapes(nested);
                quote! {
                    <<#shape as es_entity::WriteShape>::Or<<#child>::Update>
                        as es_entity::WriteShape>::Or<<#child>::Create>
                }
            },
        )
    }

    fn error_ty(
        &self,
        own_can_reject: bool,
        shape: proc_macro2::TokenStream,
    ) -> proc_macro2::TokenStream {
        let cv = self.constraint_violation();
        if own_can_reject {
            quote! { es_entity::RepoWriteError<#cv> }
        } else if !self.any_nested() {
            quote! { es_entity::RepoFault }
        } else {
            quote! { <#shape as es_entity::WriteShape>::Error<#cv> }
        }
    }

    /// The error type of the generated `create`/`create_all`.
    pub fn create_error_ty(&self) -> proc_macro2::TokenStream {
        self.error_ty(self.create_can_reject(), self.create_shape())
    }

    /// The error type of the generated `update`/`update_all`/`forget`/`delete`.
    pub fn update_error_ty(&self) -> proc_macro2::TokenStream {
        self.error_ty(self.update_can_reject(), self.update_shape())
    }

    pub fn query_fn_generics() -> proc_macro2::TokenStream {
        quote! {
            <'a, OP>
        }
    }

    pub fn query_fn_op_arg() -> proc_macro2::TokenStream {
        quote! {
            op: OP
        }
    }

    pub fn query_fn_op_traits() -> proc_macro2::TokenStream {
        quote! {
            es_entity::IntoOneTimeExecutor<'a>
        }
    }

    /// The generated `{Entity}ConstraintViolation` ident — the one repo
    /// `Rejection`. Every generated repo op returns `Result<T,
    /// es_entity::RepoWriteError<Self::ConstraintViolation>>`.
    pub fn constraint_violation(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}ConstraintViolation", self.entity_ident),
            Span::call_site(),
        )
    }

    /// The generated `{Entity}WriteError` ident: the repo's write
    /// classification (`Self::ConstraintViolation`, an optimistic-conflict
    /// retry, or an unclassified fault) made public, so a consumer
    /// hand-writing a query against the repo's tables can
    /// `.classify::<{Entity}WriteError>()?` it and get the same
    /// classification a generated write op does.
    pub fn write_error(&self) -> syn::Ident {
        syn::Ident::new(
            &format!("{}WriteError", self.entity_ident),
            Span::call_site(),
        )
    }

    /// The generated scope enum ident (`{Entity}Scope`), entity-named like
    /// the other generated companion types (`{Entity}ConstraintViolation`,
    /// `{Entity}ByIdCursor`, ...).
    pub fn scope_type_ident(&self) -> syn::Ident {
        syn::Ident::new(&format!("{}Scope", self.entity_ident), Span::call_site())
    }

    /// The generated bound-view ident (`Scoped{Repo}`), repo-named because it
    /// is a view of the repository itself (returned by `repo.scoped(scope)`).
    pub fn scoped_view_ident(&self) -> syn::Ident {
        syn::Ident::new(&format!("Scoped{}", self.ident), Span::call_site())
    }

    pub fn query_fn_get_op() -> proc_macro2::TokenStream {
        quote! {
            self.pool()
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

    pub fn validate_in_op_only(&self) -> darling::Result<()> {
        if self.pool_field().is_some() {
            return Ok(());
        }

        if !self.in_op_only {
            return Err(darling::Error::custom(
                "repo has no pool field: add a field named `pool` (or one marked \
                 `#[es_repo(pool)]`), or add `in_op_only` to #[es_repo(...)] to generate \
                 only the `_in_op` fns, which take the operation from the caller",
            )
            .with_span(&self.ident));
        }

        if !matches!(self.clock_field(), ClockFieldInfo::None) {
            return Err(darling::Error::custom(
                "repo has a clock field but no pool field: the clock is only used by \
                 `begin_op`, which an `in_op_only` repo without a pool does not generate. \
                 Remove the clock field — the operation supplies the time — or add a pool field",
            )
            .with_span(&self.ident));
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

    pub fn snapshot_enabled(&self) -> bool {
        self.snapshot
    }

    pub fn snapshot_table_name(&self) -> Option<&str> {
        if self.snapshot {
            Some(self.snapshot_table_name.as_deref().unwrap_or_else(|| {
                panic!("snapshot_table_name should have been set in update_defaults")
            }))
        } else {
            None
        }
    }

    /// `snapshot_tbl` without `snapshot` makes no sense — the table name has
    /// nothing to attach to.
    pub fn validate_snapshot(&self) -> darling::Result<()> {
        if !self.snapshot && self.snapshot_table_name.is_some() {
            return Err(darling::Error::custom("`snapshot_tbl` requires `snapshot`"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use darling::FromDeriveInput;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn snapshot_table_name_defaults_to_singular_entity_name_like_events() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Meter", snapshot)]
            struct Meters {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert_eq!(opts.events_table_name(), "meter_events");
        assert_eq!(opts.snapshot_table_name(), Some("meter_snapshots"));
    }

    #[test]
    fn snapshot_table_name_default_respects_tbl_prefix() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "InterestAccrualCycle", snapshot, tbl_prefix = "core")]
            struct InterestAccrualCycles {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert_eq!(
            opts.events_table_name(),
            "core_interest_accrual_cycle_events"
        );
        assert_eq!(
            opts.snapshot_table_name(),
            Some("core_interest_accrual_cycle_snapshots")
        );
    }

    #[test]
    fn explicit_snapshot_tbl_still_overrides_the_default() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Meter", snapshot, snapshot_tbl = "custom_meter_snaps")]
            struct Meters {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert_eq!(opts.snapshot_table_name(), Some("custom_meter_snaps"));
    }

    // No migrations directory resolves for these synthetic entities (there
    // is no real `migrations/` matching them), so `index_catalog()` is
    // empty and `update_can_reject` takes its degenerate, catalog-free
    // path: every persisted column is conservatively treated as
    // constrained, which collapses to `Columns::updates_needed()`.

    #[test]
    fn update_can_reject_when_a_column_persists_on_update() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Widget", columns(name(ty = "String")))]
            struct Widgets {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert!(opts.update_can_reject());
    }

    #[test]
    fn update_cannot_reject_when_every_column_skips_update_persist() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Widget", columns(name(ty = "String", update(persist = false))))]
            struct Widgets {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert!(!opts.update_can_reject());
    }

    #[test]
    fn a_nested_child_is_folded_into_the_error_type_by_its_own_shape() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Widget", columns(name(ty = "String", update(persist = false))))]
            struct Widgets {
                pool: sqlx::PgPool,
                #[es_repo(nested)]
                parts: WidgetParts,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert!(
            !opts.update_can_reject(),
            "the parent's own statement cannot reject"
        );
        let shape = opts.update_shape().to_string();
        assert!(
            shape.contains(
                "< WidgetPartConstraintViolation as es_entity :: WriteShapes > :: Update"
            ) && shape.contains(
                "< WidgetPartConstraintViolation as es_entity :: WriteShapes > :: Create"
            ),
            "the child's update and create shapes decide whether the parent rejects: {shape}"
        );
        assert!(
            opts.update_error_ty()
                .to_string()
                .contains("as es_entity :: WriteShape > :: Error"),
            "the error type is the folded shape's projection"
        );
    }

    #[test]
    fn a_rejecting_parent_does_not_depend_on_its_children() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Widget", columns(name(ty = "String")))]
            struct Widgets {
                pool: sqlx::PgPool,
                #[es_repo(nested)]
                parts: WidgetParts,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert_eq!(
            opts.update_error_ty().to_string(),
            "es_entity :: RepoWriteError < WidgetConstraintViolation >"
        );
    }

    #[test]
    fn update_cannot_reject_when_the_catalog_only_constrains_update_skipped_columns() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Gadget",
                migrations_dir = "tests/fixtures/catalog_authoritative",
                columns(name(ty = "String", update(persist = false)), note(ty = "String"))
            )]
            struct Gadgets {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert!(!opts.index_catalog().table_constraints("gadgets").is_empty());
        assert!(!opts.update_can_reject());
    }

    #[test]
    fn update_can_reject_when_the_catalog_constrains_an_update_persisted_column() {
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(
                entity = "Gadget",
                migrations_dir = "tests/fixtures/catalog_authoritative",
                columns(name(ty = "String"), note(ty = "String"))
            )]
            struct Gadgets {
                pool: sqlx::PgPool,
            }
        };
        let opts = RepositoryOptions::from_derive_input(&input).unwrap();
        assert!(opts.update_can_reject());
    }
}

#[cfg(test)]
mod foreign_key_tests {
    use darling::FromDeriveInput;
    use syn::parse_quote;

    use super::*;

    fn opts(columns: &str) -> RepositoryOptions {
        let columns: proc_macro2::TokenStream = columns.parse().unwrap();
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "FkWidget", migrations_dir = "tests/fixtures/fk_unique", columns(#columns))]
            struct FkWidgets {
                pool: sqlx::PgPool,
            }
        };
        RepositoryOptions::from_derive_input(&input).unwrap()
    }

    #[test]
    fn update_cannot_reject_when_only_a_foreign_key_column_persists() {
        let opts =
            opts(r#"owner_id(ty = "OwnerId"), name(ty = "String", update(persist = false))"#);
        assert!(!opts.update_can_reject());
    }

    #[test]
    fn update_can_reject_when_a_unique_column_persists_next_to_a_foreign_key() {
        let opts = opts(r#"owner_id(ty = "OwnerId"), name(ty = "String")"#);
        assert!(opts.update_can_reject());
    }

    #[test]
    fn foreign_key_has_no_violation_variant_but_is_classified_at_runtime() {
        let opts = opts(r#"owner_id(ty = "OwnerId"), name(ty = "String")"#);
        let generated = crate::repo::error_types::ErrorTypes::new(&opts)
            .generate()
            .to_string();
        assert!(!generated.contains("OwnerIdFkey"), "{generated}");
        assert!(
            generated.contains("FkWidgetNameKey") || generated.contains("NameKey"),
            "{generated}"
        );
        assert!(
            generated.contains(
                "\"foreign key violation on fk_widgets.fk_widgets_owner_id_fkey (owner_id)\""
            ),
            "{generated}"
        );
    }
}

/// One disjunct of the `create_or_find_in_op` lookup.
#[derive(Debug, Clone)]
pub struct EnsureKey {
    /// The key's columns joined by `_and_`: what `KeysSpanEntities` reports.
    pub name: String,
    pub columns: Vec<syn::Ident>,
    pub column_types: Vec<syn::Type>,
}

impl EnsureKey {
    fn of(columns: &[&Column]) -> Self {
        Self {
            name: columns
                .iter()
                .map(|c| c.name().to_string())
                .collect::<Vec<_>>()
                .join("_and_"),
            columns: columns.iter().map(|c| c.name().clone()).collect(),
            column_types: columns.iter().map(|c| c.ty().clone()).collect(),
        }
    }
}

#[cfg(test)]
mod ensure_key_tests {
    use darling::FromDeriveInput;
    use syn::parse_quote;

    use super::*;

    const COLUMNS: &str = r#"code(ty = "String"), account(ty = "String"), currency(ty = "String"), reference(ty = "String"), lowered(ty = "String")"#;

    fn options(extra: &str, columns: &str, dir: &str) -> RepositoryOptions {
        let extra: proc_macro2::TokenStream = extra.parse().unwrap();
        let columns: proc_macro2::TokenStream = columns.parse().unwrap();
        let input: syn::DeriveInput = parse_quote! {
            #[es_repo(entity = "Widget", migrations_dir = #dir, columns(#columns) #extra)]
            struct Widgets {
                pool: sqlx::PgPool,
            }
        };
        RepositoryOptions::from_derive_input(&input).unwrap()
    }

    fn names(extra: &str, columns: &str, dir: &str) -> Vec<String> {
        options(extra, columns, dir)
            .ensure_keys()
            .into_iter()
            .map(|k| k.name)
            .collect()
    }

    #[test]
    fn keys_are_the_id_then_the_declared_unique_constraints_in_catalog_order() {
        assert_eq!(
            names("", COLUMNS, "tests/fixtures/ensure_keys"),
            ["id", "code", "account_and_currency"]
        );
    }

    #[test]
    fn partial_and_expression_indexes_are_not_keys() {
        let keys = names("", COLUMNS, "tests/fixtures/ensure_keys");
        assert!(!keys.iter().any(|k| k == "reference" || k == "lowered"));
    }

    #[test]
    fn the_id_is_a_key_whoever_chooses_it() {
        assert_eq!(
            names(
                ", id(source = caller)",
                COLUMNS,
                "tests/fixtures/ensure_keys"
            ),
            names("", COLUMNS, "tests/fixtures/ensure_keys")
        );
    }

    #[test]
    fn a_column_the_create_does_not_write_is_not_a_key() {
        let columns = COLUMNS.replace(
            r#"code(ty = "String")"#,
            r#"code(ty = "String", create(persist = false))"#,
        );
        assert_eq!(
            names("", &columns, "tests/fixtures/ensure_keys"),
            ["id", "account_and_currency"]
        );
    }

    #[test]
    fn without_a_catalog_only_the_id_is_a_key() {
        assert_eq!(names("", COLUMNS, "tests/fixtures/no_such_dir"), ["id"]);
    }

    #[test]
    fn a_soft_deleting_repo_has_no_keys() {
        assert!(
            names(
                r#", delete = "soft""#,
                COLUMNS,
                "tests/fixtures/ensure_keys"
            )
            .is_empty()
        );
    }

    #[test]
    fn an_id_only_repo_cannot_reject() {
        assert!(!options("", COLUMNS, "tests/fixtures/no_such_dir").ensure_can_reject());
    }

    #[test]
    fn more_than_one_key_can_reject() {
        assert!(options("", COLUMNS, "tests/fixtures/ensure_keys").ensure_can_reject());
    }
}
