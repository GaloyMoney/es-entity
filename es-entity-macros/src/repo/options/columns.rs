use darling::FromMeta;
use quote::quote;

#[derive(Default)]
pub struct Columns {
    all: Vec<Column>,
    /// Set by an `id(scope)` entry in `columns(...)`: marks the implicit
    /// `id` column as the repo's scope column (applied in
    /// [`Self::set_id_column`]). Used by tenancy-root repos whose rows' scope
    /// value is their own id.
    id_scope: bool,
}

impl Columns {
    #[cfg(test)]
    pub fn new(id: &syn::Ident, columns: impl IntoIterator<Item = Column>) -> Self {
        let all = columns.into_iter().collect();
        let mut res = Columns {
            all,
            id_scope: false,
        };
        res.set_id_column(id);
        res
    }

    pub fn set_id_column(&mut self, ty: &syn::Ident) {
        let mut id_column = Column::for_id(syn::parse_str(&ty.to_string()).unwrap());
        id_column.opts.scope = self.id_scope;
        let mut all = vec![Column::for_created_at(), id_column];
        all.append(&mut self.all);
        self.all = all;
    }

    pub fn all_find_by(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.find_by())
    }

    pub fn all_list_by(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.list_by())
    }

    pub fn all_list_for(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| c.opts.list_for())
    }

    /// The column marked `scope`, if any. Validated by
    /// [`Self::validate_scope`] to be unique and non-nullable.
    pub fn scope_column(&self) -> Option<&Column> {
        self.all.iter().find(|c| c.opts.scope)
    }

    /// Validates the `scope` column marker:
    ///
    /// - at most one column may be marked `scope`
    /// - the scope column must be non-nullable (`Option<T>` and
    ///   `nullable`-annotated types are rejected — nullable scope columns are
    ///   a future feature)
    /// - the scope column must not be `Forgettable<T>`
    /// - the scope column must not be the `parent` column (nested repos
    ///   cannot be scoped — children are custody-guarded via their parent)
    pub fn validate_scope(&self) -> darling::Result<()> {
        let scope_columns: Vec<_> = self.all.iter().filter(|c| c.opts.scope).collect();
        if scope_columns.len() > 1 {
            return Err(darling::Error::custom(
                "only one scope column per repo is supported",
            ));
        }
        let Some(col) = scope_columns.first() else {
            return Ok(());
        };
        if col.is_nullable_column() {
            return Err(darling::Error::custom(format!(
                "scope column '{}' must be non-nullable — nullable scope columns are not supported (yet)",
                col.name(),
            )));
        }
        if col.opts.forgettable {
            return Err(darling::Error::custom(format!(
                "scope column '{}' cannot be Forgettable",
                col.name(),
            )));
        }
        if col.opts.parent_opts.is_some() {
            return Err(darling::Error::custom(format!(
                "scope column '{}' cannot be the parent column — nested repos cannot be scoped",
                col.name(),
            )));
        }
        if self.parent().is_some() {
            return Err(darling::Error::custom(
                "scope is not supported on nested repos — children are custody-guarded via their (scoped) parent",
            ));
        }
        Ok(())
    }

    pub fn find_list_by(&self, name: &syn::Ident) -> Option<&Column> {
        self.all
            .iter()
            .find(|c| c.name() == name && c.opts.list_by())
    }

    pub fn validate_list_for_by_columns(&self) -> darling::Result<()> {
        let mut errors = darling::Error::accumulator();
        for col in self.all.iter().filter(|c| c.opts.list_for()) {
            for by_name in col.list_for_by_columns() {
                if self.find_list_by(by_name).is_none() {
                    let available: Vec<_> =
                        self.all_list_by().map(|c| c.name().to_string()).collect();
                    errors.push(darling::Error::custom(format!(
                        "column '{}' in list_for(by(...)) on '{}' is not a list_by column. Available list_by columns: {}",
                        by_name,
                        col.name(),
                        available.join(", "),
                    )));
                }
            }
        }
        errors.finish()
    }

    /// Returns columns for the Column enum (id + user columns, not created_at)
    pub fn column_enum_columns(&self) -> impl Iterator<Item = &Column> {
        self.all.iter().filter(|c| *c.name() != "created_at")
    }

    pub fn parent(&self) -> Option<&Column> {
        self.all.iter().find(|c| c.opts.parent_opts.is_some())
    }

    pub fn updates_needed(&self) -> bool {
        self.all.iter().any(|c| c.opts.persist_on_update())
    }

    pub fn variable_assignments_for_update(&self, ident: syn::Ident) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_update() || c.opts.is_id {
                Some(c.variable_assignment_for_update(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn variable_assignments_for_create(&self, ident: syn::Ident) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_create() {
                Some(c.variable_assignment_for_create(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn variable_assignments_for_create_all(
        &self,
        ident: syn::Ident,
    ) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if c.opts.persist_on_create() {
                Some(c.variable_assignment_for_create_all(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    /// Eager-encoding `add` statements for the create columns, targeting a
    /// `__query_args: PgArguments`. Used by the combined index+events insert,
    /// where the event arrays require consuming the new entity before query
    /// construction — the index args are encoded first, ending their borrows.
    pub fn create_query_arg_adds(&self) -> Vec<proc_macro2::TokenStream> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .map(|column| {
                let ident = &column.name;
                let ty = &column.opts.ty;
                quote! {
                    __query_args.add(#ident as &#ty).map_err(sqlx::Error::Encode)?;
                }
            })
            .collect()
    }

    /// Per-column collection vectors for the bulk create, plus the eager
    /// `add` statements that encode them into a `__query_args: PgArguments`.
    /// Encoding eagerly (rather than binding lazily) ends the collections'
    /// borrows of the new entities, so the caller can consume them into the
    /// event stream for the same statement.
    pub fn create_all_arg_collection(
        &self,
        ident: syn::Ident,
    ) -> (proc_macro2::TokenStream, Vec<proc_macro2::TokenStream>) {
        let assignments = self.variable_assignments_for_create_all(ident.clone());
        let (vecs, pushes, bindings) = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .map(|column| {
                let vec_ident = syn::Ident::new(
                    &format!("{}_collection", column.name),
                    proc_macro2::Span::call_site(),
                );
                let ident = &column.name;
                (
                    quote! {
                        let mut #vec_ident = Vec::new();
                    },
                    quote! {
                        #vec_ident.push(#ident);
                    },
                    quote! {
                        __query_args.add(#vec_ident).map_err(sqlx::Error::Encode)?;
                    },
                )
            })
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut v1, mut v2, mut v3): (Vec<_>, Vec<_>, Vec<_>), (a, b, c)| {
                    v1.push(a);
                    v2.push(b);
                    v3.push(c);
                    (v1, v2, v3)
                },
            );
        (
            quote! {
                #(#vecs)*
                for #ident in new_entities.iter() {
                    #assignments

                    #(#pushes)*
                }
            },
            bindings,
        )
    }

    pub fn insert_column_names(&self) -> Vec<String> {
        self.all
            .iter()
            .filter_map(|c| {
                if c.opts.persist_on_create() {
                    Some(c.name.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn insert_placeholders(&self, offset: usize) -> String {
        let count = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_create())
            .count();
        ((1 + offset)..=(count + offset))
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn sql_updates(&self) -> String {
        self.all
            .iter()
            .skip(1)
            .filter(|c| c.opts.persist_on_update())
            .enumerate()
            .map(|(idx, column)| format!("{} = ${}", column.name, idx + 2))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn update_query_args(&self) -> Vec<proc_macro2::TokenStream> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|column| {
                let ident = &column.name;
                let ty = &column.opts.ty;
                quote! {
                    #ident as &#ty
                }
            })
            .collect()
    }

    /// Names of the forgettable index columns (excludes the id column).
    pub fn forgettable_column_names(&self) -> Vec<&syn::Ident> {
        self.all
            .iter()
            .filter(|c| c.opts.forgettable && !c.opts.is_id)
            .map(|c| &c.name)
            .collect()
    }

    /// `SET` clause for the soft-delete `UPDATE`, but with forgettable columns
    /// forced to `NULL` (delete auto-forgets) instead of re-persisting the live
    /// value. Placeholder numbering only advances for the non-forgettable,
    /// bound columns; it must stay in step with [`Self::update_query_args_for_delete`].
    pub fn sql_updates_for_delete(&self) -> String {
        let mut placeholder = 2;
        let mut parts = Vec::new();
        for column in self
            .all
            .iter()
            .skip(1)
            .filter(|c| c.opts.persist_on_update())
        {
            if column.opts.forgettable {
                parts.push(format!("{} = NULL", column.name));
            } else {
                parts.push(format!("{} = ${}", column.name, placeholder));
                placeholder += 1;
            }
        }
        parts.join(", ")
    }

    /// Bind args for the soft-delete `UPDATE`, excluding forgettable columns
    /// (which are literal `NULL` in [`Self::sql_updates_for_delete`]).
    pub fn update_query_args_for_delete(&self) -> Vec<proc_macro2::TokenStream> {
        self.all
            .iter()
            .filter(|c| (c.opts.persist_on_update() && !c.opts.forgettable) || c.opts.is_id)
            .map(|column| {
                let ident = &column.name;
                let ty = &column.opts.ty;
                quote! {
                    #ident as &#ty
                }
            })
            .collect()
    }

    /// Per-column `let` bindings for the soft-delete `UPDATE`, excluding
    /// forgettable columns (they are never materialised, only NULLed).
    pub fn variable_assignments_for_delete(&self, ident: syn::Ident) -> proc_macro2::TokenStream {
        let assignments = self.all.iter().filter_map(|c| {
            if (c.opts.persist_on_update() && !c.opts.forgettable) || c.opts.is_id {
                Some(c.variable_assignment_for_update(&ident))
            } else {
                None
            }
        });
        quote! {
            #(#assignments)*
        }
    }

    pub fn update_all_arg_parts(
        &self,
        ident: syn::Ident,
    ) -> (
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
        Vec<proc_macro2::TokenStream>,
    ) {
        let assignments = {
            let assignments = self.all.iter().filter_map(|c| {
                if c.opts.persist_on_update() || c.opts.is_id {
                    Some(c.variable_assignment_for_update_all(&ident))
                } else {
                    None
                }
            });
            quote! {
                #(#assignments)*
            }
        };
        let (vecs, pushes, bindings) = self
            .all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|column| {
                let vec_ident = syn::Ident::new(
                    &format!("{}_collection", column.name),
                    proc_macro2::Span::call_site(),
                );
                let ident = &column.name;
                (
                    quote! {
                        let mut #vec_ident = Vec::new();
                    },
                    quote! {
                        #vec_ident.push(#ident);
                    },
                    quote! {
                        .bind(#vec_ident)
                    },
                )
            })
            .fold(
                (Vec::new(), Vec::new(), Vec::new()),
                |(mut v1, mut v2, mut v3): (Vec<_>, Vec<_>, Vec<_>), (a, b, c)| {
                    v1.push(a);
                    v2.push(b);
                    v3.push(c);
                    (v1, v2, v3)
                },
            );
        (
            quote! { #(#vecs)* },
            quote! {
                #assignments
                #(#pushes)*
            },
            bindings,
        )
    }

    pub fn sql_bulk_update_set(&self) -> String {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() && !c.opts.is_id)
            .map(|column| format!("{name} = unnested.{name}", name = column.name))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn update_all_column_names(&self) -> Vec<String> {
        self.all
            .iter()
            .filter(|c| c.opts.persist_on_update() || c.opts.is_id)
            .map(|c| c.name.to_string())
            .collect()
    }
}

impl FromMeta for Columns {
    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let mut id_scope = false;
        let mut all = Vec::new();
        for item in items {
            if let darling::ast::NestedMeta::Meta(meta) = item
                && meta.path().is_ident("id")
            {
                if id_scope {
                    return Err(darling::Error::custom("duplicate `id` column").with_span(meta));
                }
                id_scope = id_scope_from_meta(meta)?;
                continue;
            }
            all.push(Column::from_nested_meta(item)?);
        }
        Ok(Columns { all, id_scope })
    }
}

/// The implicit `id` column is macro-owned — its type comes from the
/// repo-level `id` attribute and `find_by`/`list_by` are always on — so the
/// only supported entry is the `id(scope)` marker.
fn id_scope_from_meta(meta: &syn::Meta) -> darling::Result<bool> {
    let err =
        || darling::Error::custom("the `id` column only supports `id(scope)`").with_span(meta);
    let syn::Meta::List(list) = meta else {
        return Err(err());
    };
    let inner: syn::punctuated::Punctuated<syn::Ident, syn::Token![,]> = list
        .parse_args_with(syn::punctuated::Punctuated::parse_terminated)
        .map_err(|_| err())?;
    if inner.len() != 1 || inner[0] != "scope" {
        return Err(err());
    }
    Ok(true)
}

#[derive(PartialEq)]
pub struct Column {
    name: syn::Ident,
    opts: ColumnOpts,
}

impl FromMeta for Column {
    fn from_nested_meta(item: &darling::ast::NestedMeta) -> darling::Result<Self> {
        match item {
            darling::ast::NestedMeta::Meta(
                meta @ syn::Meta::NameValue(syn::MetaNameValue {
                    value:
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(lit_str),
                            ..
                        }),
                    ..
                }),
            ) => {
                let name = meta.path().get_ident().cloned().ok_or_else(|| {
                    darling::Error::custom("Expected identifier").with_span(meta.path())
                })?;
                Ok(Column::new(name, syn::parse_str(&lit_str.value())?))
            }
            darling::ast::NestedMeta::Meta(meta @ syn::Meta::List(_)) => {
                let name = meta.path().get_ident().cloned().ok_or_else(|| {
                    darling::Error::custom("Expected identifier").with_span(meta.path())
                })?;
                let mut opts = ColumnOpts::from_meta(meta)?;
                opts.normalize_forgettable();
                let column = Column { name, opts };
                Ok(column)
            }
            _ => Err(
                darling::Error::custom("Expected name-value pair or attribute list")
                    .with_span(item),
            ),
        }
    }
}

impl Column {
    pub fn new(name: syn::Ident, ty: syn::Type) -> Self {
        Column {
            name,
            opts: ColumnOpts::new(ty),
        }
    }

    #[cfg(test)]
    pub fn new_list_for(name: syn::Ident, ty: syn::Type, by_columns: Vec<syn::Ident>) -> Self {
        Column {
            name,
            opts: ColumnOpts {
                list_for_opts: Some(ListForOpts { by_columns }),
                ..ColumnOpts::new(ty)
            },
        }
    }

    #[cfg(test)]
    pub fn new_nullable(name: syn::Ident, ty: syn::Type) -> Self {
        Column {
            name,
            opts: ColumnOpts {
                nullable: Some(true),
                ..ColumnOpts::new(ty)
            },
        }
    }

    pub fn for_id(ty: syn::Type) -> Self {
        Column {
            name: syn::Ident::new("id", proc_macro2::Span::call_site()),
            opts: ColumnOpts {
                ty,
                is_id: true,
                forgettable: false,
                scope: false,
                list_by: Some(true),
                find_by: Some(true),
                nullable: None,
                list_for_opts: None,
                parent_opts: None,
                create_opts: Some(CreateOpts {
                    persist: Some(true),
                    accessor: None,
                }),
                update_opts: Some(UpdateOpts {
                    persist: Some(false),
                    accessor: None,
                }),
            },
        }
    }

    pub fn for_created_at() -> Self {
        Column {
            name: syn::Ident::new("created_at", proc_macro2::Span::call_site()),
            opts: ColumnOpts {
                ty: syn::parse_quote!(
                    es_entity::prelude::chrono::DateTime<es_entity::prelude::chrono::Utc>
                ),
                is_id: false,
                forgettable: false,
                scope: false,
                list_by: Some(true),
                find_by: Some(false),
                nullable: None,
                list_for_opts: None,
                parent_opts: None,
                create_opts: Some(CreateOpts {
                    persist: Some(false),
                    accessor: None,
                }),
                update_opts: Some(UpdateOpts {
                    persist: Some(false),
                    accessor: Some(syn::parse_quote!(
                        events()
                            .entity_first_persisted_at()
                            .expect("entity not persisted")
                    )),
                }),
            },
        }
    }

    pub fn list_for_by_columns(&self) -> &[syn::Ident] {
        self.opts.list_for_by_columns()
    }

    pub fn is_id(&self) -> bool {
        self.opts.is_id
    }

    /// True iff the Rust type is syntactically `Option<T>`.
    ///
    /// Drives how the macro casts query parameters and destructures the
    /// cursor — both of which depend on the Rust type's shape (Option vs.
    /// bare). For the SQL question "is this column nullable?" use
    /// [`Self::is_nullable_column`] instead.
    pub fn is_optional(&self) -> bool {
        if let syn::Type::Path(type_path) = self.ty()
            && type_path.path.segments.len() == 1
        {
            let segment = &type_path.path.segments[0];
            if segment.ident == "Option" {
                return true;
            }
        }
        false
    }

    /// True iff the underlying SQL column can hold NULL.
    ///
    /// Returns true when the Rust type is `Option<T>` *or* the column was
    /// explicitly annotated `nullable` in the repo declaration. The latter
    /// covers types like a domain enum whose custom `sqlx::Encode`
    /// implementation maps one variant to SQL NULL — the macro otherwise
    /// can't see that nullability via syntactic inspection.
    ///
    /// Drives `ORDER BY ... NULLS FIRST/LAST` emission and the nullable-aware
    /// cursor `WHERE` clause in [`Self::condition`].
    pub fn is_nullable_column(&self) -> bool {
        self.is_optional() || self.opts.nullable()
    }

    pub fn name(&self) -> &syn::Ident {
        &self.name
    }

    pub fn ty(&self) -> &syn::Type {
        &self.opts.ty
    }

    pub fn ty_for_find_by(
        &self,
    ) -> (
        syn::Type,
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
    ) {
        // A `Forgettable<Inner>` column stores `Option<Inner>` but is queried by
        // `Inner` (identical ergonomics to a naked `Inner` column). Unwrap the
        // synthesised `Option<..>` back to the inner type for the lookup param.
        let query_ty = if self.opts.forgettable {
            option_inner(self.ty()).unwrap_or_else(|| self.ty().clone())
        } else {
            self.ty().clone()
        };
        if let syn::Type::Path(type_path) = &query_ty
            && type_path.path.is_ident("String")
        {
            (
                syn::parse_quote! { str },
                quote! { impl std::convert::AsRef<str> },
                quote! { as_ref() },
            )
        } else {
            (
                query_ty.clone(),
                quote! { impl std::borrow::Borrow<#query_ty> },
                quote! { borrow() },
            )
        }
    }

    pub fn accessor(&self) -> proc_macro2::TokenStream {
        self.opts.update_accessor(&self.name)
    }

    pub fn parent_accessor(&self) -> proc_macro2::TokenStream {
        self.opts.parent_accessor(&self.name)
    }

    fn variable_assignment_for_create(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        if self.opts.forgettable {
            // The `New` entity holds the plain inner value; a freshly-created
            // entity always has the value set, so wrap it in `Some` for the
            // nullable index column. (`update`/`list_by` read the `Forgettable`
            // field of the hydrated entity via the accessor instead.)
            return quote! {
                let #name = &Some(#ident.#name.clone());
            };
        }
        let accessor = self.opts.create_accessor(name);
        quote! {
            let #name = &#ident.#accessor;
        }
    }

    fn variable_assignment_for_create_all(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let ty = &self.opts.ty;
        if self.opts.forgettable {
            return quote! {
                let #name: #ty = Some(#ident.#name.clone());
            };
        }
        let accessor = self.opts.create_accessor(name);
        if self.opts.create_accessor_returns_owned() {
            quote! {
                let #name: #ty = #ident.#accessor;
            }
        } else {
            quote! {
                let #name: &#ty = &#ident.#accessor;
            }
        }
    }

    fn variable_assignment_for_update(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.update_accessor(name);
        quote! {
            let #name = &#ident.#accessor;
        }
    }

    fn variable_assignment_for_update_all(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        let name = &self.name;
        let accessor = self.opts.update_accessor(name);
        if self.opts.update_accessor_returns_owned() {
            quote! {
                let #name = #ident.#accessor;
            }
        } else {
            quote! {
                let #name = &#ident.#accessor;
            }
        }
    }
}

/// If `ty` is `Forgettable<Inner>`, returns `Inner`; otherwise `None`.
///
/// Used to recognise `columns(name = "Forgettable<String>")`: such a column is
/// materialised into a nullable index column (`Option<Inner>`) that is queried
/// by `Inner` and set to `NULL` by `forget()`/`delete()`.
fn forgettable_inner(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Path(type_path) = ty
        && type_path.path.segments.len() == 1
    {
        let segment = &type_path.path.segments[0];
        if segment.ident == "Forgettable"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && args.args.len() == 1
            && let syn::GenericArgument::Type(inner) = &args.args[0]
        {
            return Some(inner.clone());
        }
    }
    None
}

/// If `ty` is `Option<Inner>`, returns `Inner`; otherwise `None`.
fn option_inner(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Path(type_path) = ty
        && type_path.path.segments.len() == 1
    {
        let segment = &type_path.path.segments[0];
        if segment.ident == "Option"
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
            && args.args.len() == 1
            && let syn::GenericArgument::Type(inner) = &args.args[0]
        {
            return Some(inner.clone());
        }
    }
    None
}

#[derive(PartialEq, FromMeta)]
struct ColumnOpts {
    ty: syn::Type,
    #[darling(default, skip)]
    is_id: bool,
    /// Set when the declared column type was `Forgettable<Inner>`. Such columns
    /// store the value in a nullable index column while live and are set to
    /// `NULL` by `forget()`/`delete()`. `ty` is rewritten to `Option<Inner>`.
    #[darling(default, skip)]
    forgettable: bool,
    /// Marks the repo's scope column: every generated read fn gains a leading
    /// `scope: impl Into<{Entity}Scope>` argument and filters by this column
    /// under `Only(_)`. Validated by [`Columns::validate_scope`].
    #[darling(default)]
    scope: bool,
    #[darling(default)]
    find_by: Option<bool>,
    #[darling(default)]
    list_by: Option<bool>,
    /// Opt-in flag for columns whose Rust type is not syntactically `Option<T>`
    /// but whose underlying SQL column is nullable. When set, the macro emits
    /// the same nullable-aware cursor SQL (`IS NOT DISTINCT FROM`, `NULLS
    /// FIRST/LAST` ordering, direction-aware NULL fallback) as it would for an
    /// `Option<T>` column. The Rust type itself must implement `sqlx::Encode`
    /// in a way that maps to a nullable database column — typically by
    /// encoding one variant as `IsNull::Yes` or by exposing a `Type::type_info`
    /// matching `Option<Inner>`.
    #[darling(default)]
    nullable: Option<bool>,
    #[darling(default, rename = "list_for")]
    list_for_opts: Option<ListForOpts>,
    #[darling(default, rename = "parent")]
    parent_opts: Option<ParentOpts>,
    #[darling(default, rename = "create")]
    create_opts: Option<CreateOpts>,
    #[darling(default, rename = "update")]
    update_opts: Option<UpdateOpts>,
}

impl ColumnOpts {
    fn new(ty: syn::Type) -> Self {
        let mut opts = ColumnOpts {
            ty,
            is_id: false,
            forgettable: false,
            scope: false,
            find_by: None,
            list_by: None,
            nullable: None,
            list_for_opts: None,
            parent_opts: None,
            create_opts: None,
            update_opts: None,
        };
        opts.normalize_forgettable();
        opts
    }

    /// Rewrite a `Forgettable<Inner>` column type into a nullable `Option<Inner>`
    /// index column and flag it as forgettable. Idempotent.
    fn normalize_forgettable(&mut self) {
        if let Some(inner) = forgettable_inner(&self.ty) {
            self.forgettable = true;
            self.ty = syn::parse_quote! { Option<#inner> };
        }
    }

    fn find_by(&self) -> bool {
        // `scope` flips the default to false — every read is already
        // filtered by the scope column; explicit `find_by = true` opts in.
        self.find_by.unwrap_or(!self.scope)
    }

    fn list_by(&self) -> bool {
        self.list_by.unwrap_or(false)
    }

    fn nullable(&self) -> bool {
        self.nullable.unwrap_or(false)
    }

    fn list_for(&self) -> bool {
        self.list_for_opts.is_some()
    }

    fn list_for_by_columns(&self) -> &[syn::Ident] {
        self.list_for_opts
            .as_ref()
            .map(|o| o.by_columns.as_slice())
            .unwrap_or(&[])
    }

    fn persist_on_create(&self) -> bool {
        self.create_opts
            .as_ref()
            .is_none_or(|o| o.persist.unwrap_or(true))
    }

    fn create_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if let Some(accessor) = &self.create_opts.as_ref().and_then(|o| o.accessor.as_ref()) {
            quote! {
                #accessor
            }
        } else {
            quote! {
                #name
            }
        }
    }

    fn persist_on_update(&self) -> bool {
        self.update_opts
            .as_ref()
            .is_none_or(|o| o.persist.unwrap_or(true))
    }

    fn update_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if self.forgettable {
            quote! {
                #name.value().map(|v| v.clone())
            }
        } else if let Some(accessor) = &self.update_opts.as_ref().and_then(|o| o.accessor.as_ref())
        {
            quote! {
                #accessor
            }
        } else {
            quote! {
                #name
            }
        }
    }

    fn create_accessor_returns_owned(&self) -> bool {
        self.create_opts
            .as_ref()
            .and_then(|o| o.accessor.as_ref())
            .is_some_and(|expr| matches!(expr, syn::Expr::Call(_) | syn::Expr::MethodCall(_)))
    }

    fn update_accessor_returns_owned(&self) -> bool {
        self.forgettable
            || self
                .update_opts
                .as_ref()
                .and_then(|o| o.accessor.as_ref())
                .is_some_and(|expr| matches!(expr, syn::Expr::Call(_) | syn::Expr::MethodCall(_)))
    }

    fn parent_accessor(&self, name: &syn::Ident) -> proc_macro2::TokenStream {
        if let Some(accessor) = &self.parent_opts.as_ref().and_then(|o| o.accessor.as_ref()) {
            quote! {
                #accessor
            }
        } else {
            self.update_accessor(name)
        }
    }
}

#[derive(Default, PartialEq, FromMeta)]
struct CreateOpts {
    persist: Option<bool>,
    accessor: Option<syn::Expr>,
}

#[derive(Default, PartialEq, FromMeta)]
struct UpdateOpts {
    persist: Option<bool>,
    accessor: Option<syn::Expr>,
}

#[derive(PartialEq, Debug, Default)]
struct ListForOpts {
    by_columns: Vec<syn::Ident>,
}

impl FromMeta for ListForOpts {
    fn from_word() -> darling::Result<Self> {
        Ok(ListForOpts {
            by_columns: vec![syn::Ident::new("id", proc_macro2::Span::call_site())],
        })
    }

    fn from_bool(value: bool) -> darling::Result<Self> {
        if value {
            Self::from_word()
        } else {
            Err(darling::Error::custom(
                "list_for = false is not supported; remove list_for entirely to disable",
            ))
        }
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        let mut by_columns = Vec::new();
        for item in items {
            match item {
                darling::ast::NestedMeta::Meta(syn::Meta::List(list))
                    if list.path.is_ident("by") =>
                {
                    let inner: syn::punctuated::Punctuated<syn::Ident, syn::Token![,]> =
                        list.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;
                    by_columns.extend(inner);
                }
                _ => {
                    return Err(
                        darling::Error::custom("Expected `by(col1, col2, ...)`").with_span(item)
                    );
                }
            }
        }
        Ok(ListForOpts { by_columns })
    }
}

#[derive(PartialEq, Debug, Default)]
struct ParentOpts {
    accessor: Option<syn::Expr>,
}

impl FromMeta for ParentOpts {
    fn from_word() -> darling::Result<Self> {
        Ok(ParentOpts::default())
    }

    fn from_list(items: &[darling::ast::NestedMeta]) -> darling::Result<Self> {
        #[derive(FromMeta)]
        struct Inner {
            #[darling(default)]
            accessor: Option<syn::Expr>,
        }

        let inner = Inner::from_list(items)?;
        Ok(ParentOpts {
            accessor: inner.accessor,
        })
    }
}

#[cfg(test)]
mod tests {
    use darling::FromMeta;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn column_opts_from_list() {
        let input: syn::Meta = parse_quote!(thing(
            ty = "crate::module::Thing",
            list_by = false,
            create(persist = true, accessor = accessor_fn()),
        ));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(crate::module::Thing));
        assert!(!values.list_by());
        assert!(values.find_by());
        // assert!(values.update());
        assert_eq!(
            values.create_opts.unwrap().accessor.unwrap(),
            parse_quote!(accessor_fn())
        );
    }

    #[test]
    fn columns_from_list() {
        let input: syn::Meta = parse_quote!(columns(
            name = "String",
            email(
                ty = "String",
                list_by = false,
                create(accessor = "email()"),
                update(persist = false)
            )
        ));
        let columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        assert_eq!(columns.all.len(), 2);

        assert_eq!(columns.all[0].name.to_string(), "name");

        assert_eq!(columns.all[1].name.to_string(), "email");
        assert!(!columns.all[1].opts.list_by());
        assert_eq!(
            columns.all[1]
                .opts
                .create_accessor(&parse_quote!(email))
                .to_string(),
            quote!(email()).to_string()
        );
        assert!(!columns.all[1].opts.persist_on_update());
    }

    #[test]
    fn parent_opts_from_list() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", parent));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(String));
        assert!(values.parent_opts.is_some());

        let input: syn::Meta = parse_quote!(thing(ty = "String", parent(accessor = "parent_id()")));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert_eq!(values.ty, parse_quote!(String));
        assert!(values.parent_opts.is_some());
        assert_eq!(
            values.parent_accessor(&parse_quote!(thing)).to_string(),
            quote!(parent_id()).to_string()
        );
    }

    #[test]
    fn list_for_bare_word() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 1);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "id");
    }

    #[test]
    fn list_for_with_by_columns() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for(by(created_at))));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 1);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "created_at");
    }

    #[test]
    fn list_for_by_column_must_be_list_by() {
        let id_ident: syn::Ident = parse_quote!(TestId);
        let col = Column::new_list_for(
            parse_quote!(status),
            syn::parse_str("String").unwrap(),
            vec![parse_quote!(nonexistent)],
        );
        let columns = Columns::new(&id_ident, vec![col]);
        let result = columns.validate_list_for_by_columns();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("nonexistent"),
            "error should mention the invalid column name: {err}"
        );
        assert!(
            err.contains("list_by"),
            "error should mention list_by: {err}"
        );
    }

    #[test]
    fn list_for_by_valid_column_passes_validation() {
        let id_ident: syn::Ident = parse_quote!(TestId);
        let col = Column::new_list_for(
            parse_quote!(status),
            syn::parse_str("String").unwrap(),
            vec![parse_quote!(id)],
        );
        let columns = Columns::new(&id_ident, vec![col]);
        let result = columns.validate_list_for_by_columns();
        assert!(result.is_ok());
    }

    #[test]
    fn list_for_with_multiple_by_columns() {
        let input: syn::Meta = parse_quote!(thing(ty = "String", list_for(by(created_at, id))));
        let values = ColumnOpts::from_meta(&input).expect("Failed to parse Field");
        assert!(values.list_for());
        assert_eq!(values.list_for_by_columns().len(), 2);
        assert_eq!(values.list_for_by_columns()[0].to_string(), "created_at");
        assert_eq!(values.list_for_by_columns()[1].to_string(), "id");
    }

    #[test]
    fn id_scope_from_list() {
        let input: syn::Meta = parse_quote!(columns(id(scope), name = "String"));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        assert_eq!(columns.all.len(), 1);
        columns.set_id_column(&parse_quote!(TestId));

        let scope = columns.scope_column().expect("id should be scope column");
        assert_eq!(scope.name().to_string(), "id");
        // The id column keeps its point-read and cursor fns despite being
        // the scope column (`for_id` opts in explicitly).
        assert!(scope.opts.find_by());
        assert!(scope.opts.list_by());
        assert!(columns.validate_scope().is_ok());
    }

    #[test]
    fn id_without_scope_stays_unscoped() {
        let input: syn::Meta = parse_quote!(columns(name = "String"));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&parse_quote!(TestId));
        assert!(columns.scope_column().is_none());
    }

    fn parse_columns_err(input: syn::Meta) -> String {
        match Columns::from_meta(&input) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn id_column_rejects_everything_but_scope() {
        for input in [
            parse_quote!(columns(id(ty = "TestId"), name = "String")),
            parse_quote!(columns(id = "TestId")),
            parse_quote!(columns(id(scope, find_by = false))),
        ] {
            let err = parse_columns_err(input);
            assert!(
                err.contains("id(scope)"),
                "error should point at the id(scope) form: {err}"
            );
        }
    }

    #[test]
    fn duplicate_id_column_rejected() {
        let err = parse_columns_err(parse_quote!(columns(id(scope), id(scope))));
        assert!(err.contains("duplicate"), "unexpected error: {err}");
    }

    #[test]
    fn id_scope_conflicts_with_column_scope() {
        let input: syn::Meta =
            parse_quote!(columns(id(scope), partner_id(ty = "PartnerId", scope)));
        let mut columns = Columns::from_meta(&input).expect("Failed to parse Fields");
        columns.set_id_column(&parse_quote!(TestId));
        let err = columns.validate_scope().unwrap_err().to_string();
        assert!(
            err.contains("only one scope column"),
            "unexpected error: {err}"
        );
    }
}
