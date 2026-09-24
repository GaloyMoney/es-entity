use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{
    combo_cursor::ComboCursor,
    list_by_fn::{CursorStruct, assemble_union_select, not_deleted_predicate},
    options::*,
    scope::{ScopeCol, ScopeInfo},
};

/// Runtime `Some`-ness state of one filter column. Each state that reaches
/// SQL gets its own static `es_query!` literal so that present filters
/// compile to sargable `col = $k` predicates instead of the non-sargable
/// `COALESCE(col = $k, $k IS NULL)` catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterState {
    /// Filter not applied: no predicate, no parameter.
    Absent,
    /// Non-optional column, filter applied: `col = $k`.
    Present,
    /// Optional column filtering for NULL rows: `col IS NULL`, no parameter.
    PresentNull,
    /// Optional column filtering for a value: `col = $k`.
    PresentValue,
}

impl FilterState {
    fn is_present(&self) -> bool {
        !matches!(self, FilterState::Absent)
    }
}

/// Runtime `Some`-ness state of one *bool polarity* virtual filter column
/// (`virtual = "<sql>"` with `ty = "bool"`). Unlike a physical
/// [`FilterState`], there is no notion of filtering "for NULL" — the
/// predicate is opaque SQL with no bound parameters, so each state compiles
/// to its own static conjunct: `Absent` omits it, `True` includes `(pred)`,
/// `False` includes `NOT (pred)`.
///
/// Contrast [`ValueVirtualState`], the two-state form for a *value* virtual
/// (any other `ty`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VirtualState {
    Absent,
    True,
    False,
}

impl VirtualState {
    /// Pattern element matching this state against the raw `Option<bool>`
    /// filter local (see [`ListForFiltersFn::filter_scrutinee_elems`]).
    fn pattern_elem(self) -> TokenStream {
        match self {
            VirtualState::Absent => quote! { None },
            VirtualState::True => quote! { Some(true) },
            VirtualState::False => quote! { Some(false) },
        }
    }
}

/// Runtime `Some`-ness state of one *value* virtual filter column (`virtual
/// = "<sql {value} ...>"` with a non-`bool` `ty`). There is no `NOT` state
/// and no NULL sub-state: `Absent` omits the conjunct and binds nothing;
/// `Present` includes `(pred)` with the filter value bound at its `$k`,
/// exactly like a non-optional physical `list_for` column's `col = $k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueVirtualState {
    Absent,
    Present,
}

impl ValueVirtualState {
    /// Pattern element matching this state against the `filter_{name}.is_some()`
    /// scrutinee bool (see [`ListForFiltersFn::filter_scrutinee_elems`]).
    fn pattern_elem(self) -> TokenStream {
        match self {
            ValueVirtualState::Absent => quote! { false },
            ValueVirtualState::Present => quote! { true },
        }
    }
}

/// Cartesian product of the per-column filter states.
fn filter_state_combos(columns: &[&Column]) -> Vec<Vec<FilterState>> {
    columns.iter().fold(vec![vec![]], |combos, col| {
        let options: &[FilterState] = if col.is_optional() {
            &[
                FilterState::Absent,
                FilterState::PresentValue,
                FilterState::PresentNull,
            ]
        } else {
            &[FilterState::Absent, FilterState::Present]
        };
        let mut next = Vec::with_capacity(combos.len() * options.len());
        for combo in &combos {
            for opt in options {
                let mut combo = combo.clone();
                combo.push(*opt);
                next.push(combo);
            }
        }
        next
    })
}

/// Cartesian product of the per-virtual-column states. With no virtual
/// columns this is `vec![vec![]]` — a single empty combo — so every call
/// site that loops over it degrades to exactly one (no-op) iteration,
/// keeping output byte-identical for a repo with no virtual columns.
fn virtual_state_combos(columns: &[&Column]) -> Vec<Vec<VirtualState>> {
    const STATES: [VirtualState; 3] = [
        VirtualState::Absent,
        VirtualState::True,
        VirtualState::False,
    ];
    columns.iter().fold(vec![vec![]], |combos, _| {
        let mut next = Vec::with_capacity(combos.len() * STATES.len());
        for combo in &combos {
            for state in STATES {
                let mut combo = combo.clone();
                combo.push(state);
                next.push(combo);
            }
        }
        next
    })
}

/// Cartesian product of the per-value-virtual-column states. With no value
/// virtual columns this is `vec![vec![]]` — a single empty combo — so every
/// call site that loops over it degrades to exactly one (no-op) iteration,
/// keeping output byte-identical for a repo with no value virtual columns
/// (including one with only bool-polarity virtuals, per #236).
fn value_virtual_state_combos(columns: &[&Column]) -> Vec<Vec<ValueVirtualState>> {
    const STATES: [ValueVirtualState; 2] = [ValueVirtualState::Absent, ValueVirtualState::Present];
    columns.iter().fold(vec![vec![]], |combos, _| {
        let mut next = Vec::with_capacity(combos.len() * STATES.len());
        for combo in &combos {
            for state in STATES {
                let mut combo = combo.clone();
                combo.push(state);
                next.push(combo);
            }
        }
        next
    })
}

/// Local variable name a bool-polarity virtual column's raw `Option<bool>`
/// filter value is destructured into: `filters.flagged` -> `let
/// virtual_flagged = ...`. A value virtual instead destructures into
/// `filter_{name}` — the same local-naming convention a physical `list_for`
/// column uses — since it binds through the same [`FiltersStruct::filter_arg_tokens`]
/// path.
fn virtual_local_ident(name: &syn::Ident) -> syn::Ident {
    syn::Ident::new(&format!("virtual_{name}"), Span::call_site())
}

/// The static SQL conjuncts contributed by one bool-polarity virtual-state
/// combo: `(pred)` for `True`, `NOT (pred)` for `False`, nothing for
/// `Absent`. Meant to be appended to the `trailing` conjuncts of
/// [`assemble_union_select`] — same place the soft-delete `deleted = FALSE`
/// predicate lands, since both are unparameterized. `columns` must be the
/// bool-polarity virtuals only (see [`ListForFiltersFn::bool_virtuals`]) —
/// value virtuals contribute their `(pred)` conjunct separately, alongside
/// their bound `$k`.
fn virtual_trailing(columns: &[&Column], combo: &[VirtualState]) -> Vec<String> {
    columns
        .iter()
        .zip(combo.iter())
        .filter_map(|(col, state)| {
            let pred = col.virtual_predicate();
            match state {
                VirtualState::Absent => None,
                VirtualState::True => Some(format!("({pred})")),
                VirtualState::False => Some(format!("NOT ({pred})")),
            }
        })
        .collect()
}

pub struct FiltersStruct<'a> {
    columns: Vec<&'a Column>,
    virtual_columns: Vec<&'a Column>,
    entity: &'a syn::Ident,
}

impl<'a> FiltersStruct<'a> {
    pub fn new(
        opts: &'a RepositoryOptions,
        columns: Vec<&'a Column>,
        virtual_columns: Vec<&'a Column>,
    ) -> Self {
        Self {
            entity: opts.entity(),
            columns,
            virtual_columns,
        }
    }

    #[cfg(test)]
    fn new_test(entity: &'a syn::Ident, columns: Vec<&'a Column>) -> Self {
        Self {
            entity,
            columns,
            virtual_columns: Vec::new(),
        }
    }

    #[cfg(test)]
    fn new_test_with_virtual(
        entity: &'a syn::Ident,
        columns: Vec<&'a Column>,
        virtual_columns: Vec<&'a Column>,
    ) -> Self {
        Self {
            entity,
            columns,
            virtual_columns,
        }
    }

    pub fn ident(&self) -> syn::Ident {
        let entity_name = format!("{}", self.entity);
        syn::Ident::new(
            &format!("{entity_name}_filters").to_case(Case::UpperCamel),
            Span::call_site(),
        )
    }

    fn fields(&self) -> TokenStream {
        self.columns
            .iter()
            .chain(self.virtual_columns.iter())
            .map(|column| {
                let name = column.name();
                let ty = column.ty();
                quote! {
                    pub #name: Option<#ty>,
                }
            })
            .collect()
    }

    fn where_clause_fragment(column: &Column, param_idx: &mut u32) -> String {
        let col_name = column.name();
        if column.is_optional() {
            let apply_param = format!("${}", *param_idx);
            *param_idx += 1;
            let val_param = format!("${}", *param_idx);
            *param_idx += 1;
            format!("(NOT {apply_param} OR {col_name} IS NOT DISTINCT FROM {val_param})")
        } else {
            let param = format!("${}", *param_idx);
            *param_idx += 1;
            format!("COALESCE({col_name} = {param}, {param} IS NULL)")
        }
    }

    fn filter_arg_tokens(column: &Column) -> TokenStream {
        let col_name = column.name();
        let filter_name = syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
        let ty = column.ty();
        if column.is_optional() {
            let apply_name = syn::Ident::new(&format!("apply_{}", col_name), Span::call_site());
            quote! {
                #apply_name as bool,
                #filter_name as #ty,
            }
        } else if let syn::Type::Path(type_path) = ty
            && type_path.path.is_ident("String")
        {
            quote! {
                #filter_name as Option<String>,
            }
        } else {
            quote! {
                #filter_name as Option<#ty>,
            }
        }
    }

    /// Value-only binding for an optional column in a specialized
    /// `col = $k` variant (the `apply` flag is encoded in the variant
    /// itself, so only the value parameter remains).
    fn filter_value_arg_tokens(column: &Column) -> TokenStream {
        let col_name = column.name();
        let filter_name = syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
        let ty = column.ty();
        quote! {
            #filter_name as #ty,
        }
    }
}

impl ToTokens for FiltersStruct<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = self.ident();
        let fields = self.fields();

        tokens.append_all(quote! {
            #[derive(Debug, Default)]
            pub struct #ident {
                #fields
            }
        });
    }
}

pub struct ListForFiltersFn<'a> {
    in_op_only: bool,
    pub filters_struct: FiltersStruct<'a>,
    entity: &'a syn::Ident,
    query_error: syn::Ident,
    for_columns: Vec<&'a Column>,
    virtual_columns: Vec<&'a Column>,
    by_columns: Vec<&'a Column>,
    cursor: &'a ComboCursor<'a>,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    table_name: &'a str,
    ignore_prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    post_hydrate_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    snapshot_table_name: Option<&'a str>,
    scope: Option<ScopeInfo<'a>>,
    index_catalog: crate::index_catalog::IndexCatalog,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListForFiltersFn<'a> {
    pub fn new(
        opts: &'a RepositoryOptions,
        for_columns: Vec<&'a Column>,
        virtual_columns: Vec<&'a Column>,
        by_columns: Vec<&'a Column>,
        cursor: &'a ComboCursor<'a>,
    ) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            filters_struct: FiltersStruct::new(opts, for_columns.clone(), virtual_columns.clone()),
            entity: opts.entity(),
            query_error: opts.query_error(),
            for_columns,
            virtual_columns,
            by_columns,
            cursor,
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            table_name: opts.table_name(),
            ignore_prefix: opts.table_prefix(),
            id: opts.id(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            snapshot_table_name: opts.snapshot_table_name(),
            scope: ScopeInfo::from_opts(opts),
            index_catalog: opts.index_catalog(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    /// The bool-polarity virtual columns (`ty = "bool"`), in declaration
    /// order — see [`VirtualState`].
    fn bool_virtuals(&self) -> Vec<&'a Column> {
        self.virtual_columns
            .iter()
            .copied()
            .filter(|c| !c.is_value_virtual())
            .collect()
    }

    /// The value virtual columns (non-`bool` `ty`), in declaration order —
    /// see [`ValueVirtualState`].
    fn value_virtuals(&self) -> Vec<&'a Column> {
        self.virtual_columns
            .iter()
            .copied()
            .filter(|c| c.is_value_virtual())
            .collect()
    }

    /// Delegating methods for the generated `Scoped{Repo}` bound view.
    /// Empty for unscoped repos.
    pub fn scoped_delegates(&self) -> TokenStream {
        let mut tokens = TokenStream::new();
        if self.scope.is_none() {
            return tokens;
        }
        let entity = self.entity;
        let error = &self.query_error;
        let cursor_mod = &self.cursor_mod;
        let filters_ident = self.filters_struct.ident();
        let sort_by_name = self.cursor.sort_by_name();
        let combo_cursor_ident = self.cursor.ident();
        let query_fn_generics = RepositoryOptions::query_fn_generics();
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg();
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits();

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let delete_postfix = delete.include_deletion_fn_postfix();

            for by_column in &self.by_columns {
                let cursor_struct = CursorStruct {
                    column: by_column,
                    id: self.id,
                    entity: self.entity,
                    cursor_mod: &self.cursor_mod,
                };
                let cursor_ident = cursor_struct.ident();
                let fn_name = syn::Ident::new(
                    &format!("list_for_filters_by_{}{}", by_column.name(), delete_postfix),
                    Span::call_site(),
                );
                let fn_in_op = syn::Ident::new(
                    &format!(
                        "list_for_filters_by_{}{}_in_op",
                        by_column.name(),
                        delete_postfix
                    ),
                    Span::call_site(),
                );

                let standalone = (!self.in_op_only).then(|| {
                    quote! {
                        pub async fn #fn_name(
                            &self,
                            filters: #filters_ident,
                            cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                            direction: es_entity::ListDirection,
                        ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                            self.repo.#fn_name(self.scope, filters, cursor, direction).await
                        }
                    }
                });

                tokens.append_all(quote! {
                    #standalone

                    pub async fn #fn_in_op #query_fn_generics(
                        &self,
                        #query_fn_op_arg,
                        filters: #filters_ident,
                        cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                        direction: es_entity::ListDirection,
                    ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                        where
                            OP: #query_fn_op_traits
                    {
                        self.repo.#fn_in_op(op, self.scope, filters, cursor, direction).await
                    }
                });
            }

            let dispatch_fn = syn::Ident::new(
                &format!("list_for_filters{delete_postfix}"),
                Span::call_site(),
            );
            let dispatch_fn_in_op = syn::Ident::new(
                &format!("list_for_filters{delete_postfix}_in_op"),
                Span::call_site(),
            );
            let standalone_dispatch = (!self.in_op_only).then(|| {
                quote! {
                    pub async fn #dispatch_fn(
                        &self,
                        filters: #filters_ident,
                        sort: es_entity::Sort<#sort_by_name>,
                        cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#combo_cursor_ident>,
                    ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#combo_cursor_ident>, #error> {
                        self.repo.#dispatch_fn(self.scope, filters, sort, cursor).await
                    }
                }
            });
            tokens.append_all(quote! {
                #standalone_dispatch

                pub async fn #dispatch_fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    filters: #filters_ident,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#combo_cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#combo_cursor_ident>, #error>
                    where
                        OP: #query_fn_op_traits
                {
                    self.repo.#dispatch_fn_in_op(op, self.scope, filters, sort, cursor).await
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
        tokens
    }

    /// Scrutinee elements (bools over the destructured filter locals)
    /// identifying each filter's state at runtime: one bool per non-optional
    /// physical column (`is_some`), two per optional physical column
    /// (`apply`, value `is_some`), one raw `Option<bool>` per bool-polarity
    /// virtual column (matched directly against `None` / `Some(true)` /
    /// `Some(false)` — see [`VirtualState::pattern_elem`]), and one bool per
    /// value virtual column (`filter_{name}.is_some()`, matched against
    /// `true`/`false` — see [`ValueVirtualState::pattern_elem`]).
    fn filter_scrutinee_elems(&self) -> Vec<TokenStream> {
        let mut elems: Vec<TokenStream> = self
            .for_columns
            .iter()
            .flat_map(|c| {
                let col_name = c.name();
                let filter_name =
                    syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
                if c.is_optional() {
                    let apply_name =
                        syn::Ident::new(&format!("apply_{}", col_name), Span::call_site());
                    vec![quote! { #apply_name }, quote! { #filter_name.is_some() }]
                } else {
                    vec![quote! { #filter_name.is_some() }]
                }
            })
            .collect();
        elems.extend(
            self.bool_virtuals()
                .into_iter()
                .map(|c| virtual_local_ident(c.name()))
                .map(|local| quote! { #local }),
        );
        elems.extend(self.value_virtuals().into_iter().map(|c| {
            let filter_name = syn::Ident::new(&format!("filter_{}", c.name()), Span::call_site());
            quote! { #filter_name.is_some() }
        }));
        elems
    }

    /// Pattern elements matching [`Self::filter_scrutinee_elems`] for one
    /// column in one state.
    fn filter_pattern_elems(column: &Column, state: FilterState) -> Vec<TokenStream> {
        if column.is_optional() {
            match state {
                FilterState::Absent => vec![quote! { false }, quote! { _ }],
                FilterState::PresentValue => vec![quote! { true }, quote! { true }],
                FilterState::PresentNull => vec![quote! { true }, quote! { false }],
                FilterState::Present => unreachable!("optional columns split Present"),
            }
        } else {
            match state {
                FilterState::Absent => vec![quote! { false }],
                FilterState::Present => vec![quote! { true }],
                _ => unreachable!("non-optional columns have no NULL sub-state"),
            }
        }
    }

    /// Whether a filter combination, paginated by `by_column`, gets a
    /// specialized sargable query — decided purely by the physical index
    /// catalog (derived from the migrations). A combination is specialized iff
    /// some composite index's leading key columns are a permutation of the
    /// equality columns (the scope arm's column, when present, plus every
    /// constrained filter — `= $k` *and* `IS NULL` states both constrain the
    /// column) immediately followed by the sort column. Everything else falls
    /// back to the correct (non-sargable) `COALESCE` query. No arity cap:
    /// build cost tracks declared indexes, not `3^n` combinations.
    ///
    /// Each scope-column arm is checked independently against the index
    /// catalog: a repo may have partner-led composite indexes but no
    /// customer-led ones, in which case the partner arm specializes while
    /// the customer arm honestly falls back.
    fn is_specialized_combo(
        &self,
        combo: &[FilterState],
        by_column: &Column,
        scope: Option<&ScopeCol>,
    ) -> bool {
        let mut equality_cols: Vec<String> = Vec::new();
        if let Some(scope) = scope {
            equality_cols.push(scope.column_name.to_string());
        }
        for (col, state) in self.for_columns.iter().zip(combo.iter()) {
            if state.is_present() {
                equality_cols.push(col.name().to_string());
            }
        }
        self.index_catalog.specializes(
            self.table_name,
            &equality_cols,
            &by_column.name().to_string(),
        )
    }

    fn generate_proxy_body(
        &self,
        by_col: &Column,
        delete: DeleteOption,
        in_op: bool,
    ) -> TokenStream {
        let by_col_name = by_col.name();
        let delete_postfix = delete.include_deletion_fn_postfix();

        let scope_pass = if self.scope.is_some() {
            quote! { __scope, }
        } else {
            quote! {}
        };

        let in_op_postfix = if in_op { "_in_op" } else { "" };
        let op_pass = if in_op {
            quote! { op, }
        } else {
            quote! {}
        };

        let list_by_fn = syn::Ident::new(
            &format!("list_by_{by_col_name}{delete_postfix}{in_op_postfix}"),
            Span::call_site(),
        );

        if self.for_columns.is_empty() && self.virtual_columns.is_empty() {
            return quote! { self.#list_by_fn(#op_pass #scope_pass query, direction).await? };
        }

        let virtual_none_checks: Vec<TokenStream> = self
            .virtual_columns
            .iter()
            .map(|c| {
                let name = c.name();
                quote! { filters.#name.is_none() }
            })
            .collect();

        let all_none_checks: Vec<_> = self
            .for_columns
            .iter()
            .map(|c| {
                let name = c.name();
                quote! { filters.#name.is_none() }
            })
            .chain(virtual_none_checks.iter().cloned())
            .collect();

        // Determine which for_columns have individual methods for this by_col.
        let paired_for_columns: Vec<_> = self
            .for_columns
            .iter()
            .filter(|fc| fc.list_for_by_columns().iter().any(|n| n == by_col_name))
            .collect();

        let single_filter_branches: TokenStream = paired_for_columns
            .iter()
            .map(|for_col| {
                // Every other physical filter, and — critically — every
                // virtual filter, must also be absent: a virtual `Some`
                // always needs the unified conjunct path, never the
                // dedicated per-column fn.
                let others_none: Vec<_> = self
                    .for_columns
                    .iter()
                    .filter(|c| c.name() != for_col.name())
                    .map(|c| {
                        let name = c.name();
                        quote! { filters.#name.is_none() }
                    })
                    .chain(virtual_none_checks.iter().cloned())
                    .collect();

                let for_col_name = for_col.name();
                let fn_name = syn::Ident::new(
                    &format!(
                        "list_for_{for_col_name}_by_{by_col_name}{delete_postfix}{in_op_postfix}"
                    ),
                    Span::call_site(),
                );

                if others_none.is_empty() {
                    quote! {
                        else {
                            self.#fn_name(#op_pass #scope_pass filters.#for_col_name.unwrap(), query, direction).await?
                        }
                    }
                } else {
                    quote! {
                        else if #(#others_none)&&* {
                            self.#fn_name(#op_pass #scope_pass filters.#for_col_name.unwrap(), query, direction).await?
                        }
                    }
                }
            })
            .collect();

        // Need a fallback when:
        // - there are unpaired for_columns (they need COALESCE)
        // - there are 2+ paired columns (multi-filter case)
        // - there are 2+ for_columns total (multi-filter case)
        // - there is any virtual column: it always routes through the
        //   unified conjunct path, never a per-column shortcut
        let has_unpaired = paired_for_columns.len() < self.for_columns.len();
        let needs_fallback =
            has_unpaired || self.for_columns.len() >= 2 || !self.virtual_columns.is_empty();
        let multi_filter_fallback = if needs_fallback {
            let list_for_filters_fn = syn::Ident::new(
                &format!("list_for_filters_by_{by_col_name}{delete_postfix}{in_op_postfix}"),
                Span::call_site(),
            );
            quote! {
                else {
                    self.#list_for_filters_fn(#op_pass #scope_pass filters, query, direction).await?
                }
            }
        } else {
            quote! {}
        };

        quote! {
            if #(#all_none_checks)&&* {
                self.#list_by_fn(#op_pass #scope_pass query, direction).await?
            }
            #single_filter_branches
            #multi_filter_fallback
        }
    }

    fn generate_by_fn(&self, by_column: &'a Column, delete: DeleteOption) -> TokenStream {
        let entity = self.entity;
        let error = &self.query_error;
        let cursor_mod = &self.cursor_mod;
        let query_fn_generics = RepositoryOptions::query_fn_generics();
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg();
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits();
        let query_fn_get_op = RepositoryOptions::query_fn_get_op();

        let by_column_name = by_column.name();
        let cursor_struct = CursorStruct {
            column: by_column,
            id: self.id,
            entity: self.entity,
            cursor_mod: &self.cursor_mod,
        };
        let cursor_ident = cursor_struct.ident();

        let destructure_tokens = cursor_struct.destructure_tokens();
        let select_columns = cursor_struct.select_columns(None);
        let cursor_arg_tokens = cursor_struct.query_arg_tokens();

        let fn_name = syn::Ident::new(
            &format!(
                "list_for_filters_by_{}{}",
                by_column_name,
                delete.include_deletion_fn_postfix()
            ),
            Span::call_site(),
        );
        let fn_in_op = syn::Ident::new(
            &format!(
                "list_for_filters_by_{}{}_in_op",
                by_column_name,
                delete.include_deletion_fn_postfix()
            ),
            Span::call_site(),
        );

        let filters_ident = self.filters_struct.ident();

        // Generate filter destructuring
        let destructure_filters: TokenStream = self
            .for_columns
            .iter()
            .map(|c| {
                let col_name = c.name();
                let filter_name =
                    syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
                if c.is_optional() {
                    let apply_name =
                        syn::Ident::new(&format!("apply_{}", col_name), Span::call_site());
                    quote! {
                        let #apply_name = filters.#col_name.is_some();
                        let #filter_name = filters.#col_name.flatten();
                    }
                } else {
                    quote! {
                        let #filter_name = filters.#col_name;
                    }
                }
            })
            .chain(self.bool_virtuals().into_iter().map(|c| {
                let col_name = c.name();
                let local = virtual_local_ident(col_name);
                quote! {
                    let #local = filters.#col_name;
                }
            }))
            .chain(self.value_virtuals().into_iter().map(|c| {
                let col_name = c.name();
                let filter_name =
                    syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
                quote! {
                    let #filter_name = filters.#col_name;
                }
            }))
            .collect();

        // Generate the non-specialized fallback query: correct for every filter
        // combination, sargable only where a matching composite index exists.
        // The filter predicates (COALESCE / apply-flag forms) are the leading
        // conjuncts shared by every unified-cursor `UNION ALL` branch.
        // Parameterized over the scope: each scope-column arm binds its
        // column at `$1` and shifts every other parameter by one.
        let build_fallback = |scope: Option<&ScopeCol>,
                              virtual_combo: &[VirtualState],
                              value_combo: &[ValueVirtualState]|
         -> (String, String, TokenStream) {
            let scope_offset: u32 = if scope.is_some() { 1 } else { 0 };
            let mut param_idx = 1u32 + scope_offset;
            let where_fragments: Vec<String> = self
                .for_columns
                .iter()
                .map(|col| FiltersStruct::where_clause_fragment(col, &mut param_idx))
                .collect();

            let mut leading: Vec<String> = Vec::new();
            if let Some(scope) = scope {
                leading.push(scope.predicate(1));
            }
            leading.extend(where_fragments);

            let mut trailing: Vec<String> = Vec::new();
            if delete == DeleteOption::No
                && let Some(not_deleted) = not_deleted_predicate(self.delete)
            {
                trailing.push(not_deleted);
            }
            trailing.extend(virtual_trailing(&self.bool_virtuals(), virtual_combo));

            let mut filter_arg_bindings: TokenStream = self
                .for_columns
                .iter()
                .map(|col| FiltersStruct::filter_arg_tokens(col))
                .collect();

            // Value virtuals: each `Present` column allocates its `$k`
            // immediately after the physical filter params and before
            // limit/cursor, in declaration order — the substituted
            // predicate joins `trailing` and its bound arg joins the args
            // token stream in the same order, so `param_idx` (which then
            // seeds limit/cursor numbering) shifts correctly for free.
            for (col, state) in self.value_virtuals().iter().zip(value_combo.iter()) {
                if *state == ValueVirtualState::Present {
                    let pred = col.virtual_predicate();
                    let substituted = substitute_value_placeholder(pred, param_idx).expect(
                        "value virtual predicate was validated to contain {value} at derive time",
                    );
                    trailing.push(format!("({substituted})"));
                    param_idx += 1;
                    filter_arg_bindings.append_all(FiltersStruct::filter_arg_tokens(col));
                }
            }

            let scope_args = scope.map(|s| s.arg_tokens()).unwrap_or_default();
            let fallback_arg_tokens = quote! {
                #scope_args
                #filter_arg_bindings
                #cursor_arg_tokens
            };

            let asc_query = assemble_union_select(
                &select_columns,
                self.table_name,
                &leading,
                &cursor_struct.cursor_branches(param_idx - 1, true),
                &trailing,
                &cursor_struct.order_by(true),
                param_idx,
            );
            let desc_query = assemble_union_select(
                &select_columns,
                self.table_name,
                &leading,
                &cursor_struct.cursor_branches(param_idx - 1, false),
                &trailing,
                &cursor_struct.order_by(false),
                param_idx,
            );
            (asc_query, desc_query, fallback_arg_tokens)
        };
        let forgettable_tbl_arg = if let Some(tbl) = self.forgettable_table_name {
            quote! { forgettable_tbl = #tbl, }
        } else {
            quote! {}
        };
        let snapshot_tbl_arg = if let Some(tbl) = self.snapshot_table_name {
            quote! { snapshot_tbl = #tbl, }
        } else {
            quote! {}
        };

        let make_es_query = |query: &str, args: &TokenStream| -> TokenStream {
            if let Some(prefix) = self.ignore_prefix {
                quote! {
                    es_entity::es_query!(
                        tbl_prefix = #prefix,
                        #forgettable_tbl_arg
                        #snapshot_tbl_arg
                        #query,
                        #args
                    )
                }
            } else {
                quote! {
                    es_entity::es_query!(
                        entity = #entity,
                        #forgettable_tbl_arg
                        #snapshot_tbl_arg
                        #query,
                        #args
                    )
                }
            }
        };

        // Specialized variant matrix: one static query per (filter
        // combination x cursor state x direction). Every present filter
        // compiles to a sargable `col = $k` (or `col IS NULL`) predicate and
        // the cursor predicate is either omitted (page 1) or a bare row
        // comparison. Parameterized over the scope: each scope-column arm
        // binds its column at `$1` and shifts every other parameter by one.
        let bool_virtuals = self.bool_virtuals();
        let value_virtuals = self.value_virtuals();
        let build_specialized_arms =
            |scope: Option<&ScopeCol>| -> (TokenStream, TokenStream, bool) {
                let mut asc_arms = TokenStream::new();
                let mut desc_arms = TokenStream::new();
                let mut all_combos_specialized = true;
                // Virtual state is orthogonal to physical specialization (it
                // is never an equality column considered by the index
                // catalog), so it wraps the physical combo loop: every
                // specialized physical combo gets one explicit arm per
                // (bool-virtual-state x value-virtual-state) combo. With no
                // virtual columns of either kind both outer loops run
                // exactly once with an empty combo, reproducing the prior
                // output byte-for-byte.
                for virtual_combo in virtual_state_combos(&bool_virtuals) {
                    let virtual_patterns: Vec<TokenStream> =
                        virtual_combo.iter().map(|s| s.pattern_elem()).collect();
                    let virtual_trailing_conds = virtual_trailing(&bool_virtuals, &virtual_combo);

                    for value_combo in value_virtual_state_combos(&value_virtuals) {
                        let value_patterns: Vec<TokenStream> =
                            value_combo.iter().map(|s| s.pattern_elem()).collect();

                        for combo in filter_state_combos(&self.for_columns) {
                            if !self.is_specialized_combo(&combo, cursor_struct.column, scope) {
                                all_combos_specialized = false;
                                continue;
                            }
                            let filter_patterns: Vec<TokenStream> = self
                                .for_columns
                                .iter()
                                .zip(combo.iter())
                                .flat_map(|(col, state)| Self::filter_pattern_elems(col, *state))
                                .collect();

                            let mut filter_conditions: Vec<String> = Vec::new();
                            let mut filter_args = TokenStream::new();
                            let mut param_idx = 1u32;
                            if let Some(scope) = scope {
                                filter_conditions.push(scope.predicate(1));
                                filter_args.append_all(scope.arg_tokens());
                                param_idx += 1;
                            }
                            for (col, state) in self.for_columns.iter().zip(combo.iter()) {
                                match state {
                                    FilterState::Absent => {}
                                    FilterState::Present => {
                                        filter_conditions.push(format!(
                                            "{} = ${}",
                                            col.name(),
                                            param_idx
                                        ));
                                        param_idx += 1;
                                        filter_args
                                            .append_all(FiltersStruct::filter_arg_tokens(col));
                                    }
                                    FilterState::PresentNull => {
                                        filter_conditions.push(format!("{} IS NULL", col.name()));
                                    }
                                    FilterState::PresentValue => {
                                        filter_conditions.push(format!(
                                            "{} = ${}",
                                            col.name(),
                                            param_idx
                                        ));
                                        param_idx += 1;
                                        filter_args.append_all(
                                            FiltersStruct::filter_value_arg_tokens(col),
                                        );
                                    }
                                }
                            }

                            // Value virtuals: allocate each `Present`
                            // column's `$k` after the physical filter
                            // params, before limit/cursor — same rule as
                            // `build_fallback`.
                            let mut value_trailing_conds: Vec<String> = Vec::new();
                            for (col, vstate) in value_virtuals.iter().zip(value_combo.iter()) {
                                if *vstate == ValueVirtualState::Present {
                                    let pred = col.virtual_predicate();
                                    let substituted = substitute_value_placeholder(
                                        pred, param_idx,
                                    )
                                    .expect(
                                        "value virtual predicate was validated to contain {value} at derive time",
                                    );
                                    value_trailing_conds.push(format!("({substituted})"));
                                    param_idx += 1;
                                    filter_args.append_all(FiltersStruct::filter_arg_tokens(col));
                                }
                            }

                            // The cursor-state dimension collapses into one unified
                            // `UNION ALL` query per direction (the specialized filter
                            // predicates are the leading conjuncts of every branch), so
                            // the arm matches on the filter scrutinee alone.
                            let pattern = quote! {
                                (#(#filter_patterns,)* #(#virtual_patterns,)* #(#value_patterns,)*)
                            };
                            let cursor_args = cursor_struct.cursor_arg_tokens();
                            let args = quote! {
                                #filter_args
                                (first + 1) as i64,
                                #cursor_args
                            };

                            let mut trailing: Vec<String> = Vec::new();
                            if delete == DeleteOption::No
                                && let Some(not_deleted) = not_deleted_predicate(self.delete)
                            {
                                trailing.push(not_deleted);
                            }
                            trailing.extend(virtual_trailing_conds.iter().cloned());
                            trailing.extend(value_trailing_conds);

                            for ascending in [true, false] {
                                let query = assemble_union_select(
                                    &select_columns,
                                    self.table_name,
                                    &filter_conditions,
                                    &cursor_struct.cursor_branches(param_idx - 1, ascending),
                                    &trailing,
                                    &cursor_struct.order_by(ascending),
                                    param_idx,
                                );
                                let es_query_call = make_es_query(&query, &args);
                                if ascending {
                                    asc_arms.append_all(quote! {
                                        #pattern => {
                                            #es_query_call.fetch_n(op, first).await?
                                        },
                                    });
                                } else {
                                    desc_arms.append_all(quote! {
                                        #pattern => {
                                            #es_query_call.fetch_n(op, first).await?
                                        },
                                    });
                                }
                            }
                        }
                    }
                }
                (asc_arms, desc_arms, all_combos_specialized)
            };
        let (asc_arms, desc_arms, all_combos_specialized) = build_specialized_arms(None);

        let scrutinee_elems: Vec<TokenStream> = self.filter_scrutinee_elems();

        // When every filter combination is specialized the explicit arms
        // already cover the entire pattern space, so no wildcard fallback arm
        // (nor its catch-all COALESCE queries) is emitted. With no virtual
        // columns this is the single `_ =>` catch-all as before; with
        // virtual columns — whose predicate is opaque SQL binding no
        // parameters, so it cannot ride the same COALESCE trick physical
        // optional columns use — it becomes one static fallback query per
        // virtual state combo, wildcarding the (already COALESCE-based)
        // physical dimension via a `..` rest pattern.
        let build_fallback_arms = |scope: Option<&ScopeCol>,
                                   all_specialized: bool|
         -> (TokenStream, TokenStream) {
            if all_specialized {
                return (quote! {}, quote! {});
            }
            if bool_virtuals.is_empty() && value_virtuals.is_empty() {
                let (asc_query, desc_query, args) = build_fallback(scope, &[], &[]);
                let asc_call = make_es_query(&asc_query, &args);
                let desc_call = make_es_query(&desc_query, &args);
                return (
                    quote! { _ => #asc_call.fetch_n(op, first).await?, },
                    quote! { _ => #desc_call.fetch_n(op, first).await?, },
                );
            }
            let mut asc_arms = TokenStream::new();
            let mut desc_arms = TokenStream::new();
            for virtual_combo in virtual_state_combos(&bool_virtuals) {
                for value_combo in value_virtual_state_combos(&value_virtuals) {
                    let (asc_query, desc_query, args) =
                        build_fallback(scope, &virtual_combo, &value_combo);
                    let asc_call = make_es_query(&asc_query, &args);
                    let desc_call = make_es_query(&desc_query, &args);
                    let virtual_patterns: Vec<TokenStream> =
                        virtual_combo.iter().map(|s| s.pattern_elem()).collect();
                    let value_patterns: Vec<TokenStream> =
                        value_combo.iter().map(|s| s.pattern_elem()).collect();
                    let pattern = quote! { (.., #(#virtual_patterns,)* #(#value_patterns,)*) };
                    asc_arms
                        .append_all(quote! { #pattern => #asc_call.fetch_n(op, first).await?, });
                    desc_arms
                        .append_all(quote! { #pattern => #desc_call.fetch_n(op, first).await?, });
                }
            }
            (asc_arms, desc_arms)
        };
        let (asc_fallback_arm, desc_fallback_arm) =
            build_fallback_arms(None, all_combos_specialized);

        let direction_match = |asc_arms: &TokenStream,
                               asc_fallback: &TokenStream,
                               desc_arms: &TokenStream,
                               desc_fallback: &TokenStream|
         -> TokenStream {
            quote! {
                match direction {
                    es_entity::ListDirection::Ascending => match (#(#scrutinee_elems,)*) {
                        #asc_arms
                        #asc_fallback
                    },
                    es_entity::ListDirection::Descending => match (#(#scrutinee_elems,)*) {
                        #desc_arms
                        #desc_fallback
                    }
                }
            }
        };
        let (scope_fn_arg, scope_fn_pass, scope_convert) = match &self.scope {
            Some(scope) => (scope.fn_arg(), scope.fn_pass(), scope.convert()),
            None => (quote! {}, quote! {}, quote! {}),
        };
        let match_expr = if let Some(scope) = &self.scope {
            scope.dispatch(
                direction_match(&asc_arms, &asc_fallback_arm, &desc_arms, &desc_fallback_arm),
                |col| {
                    let (scoped_asc_arms, scoped_desc_arms, scoped_all_specialized) =
                        build_specialized_arms(Some(col));
                    let (scoped_asc_fallback, scoped_desc_fallback) =
                        build_fallback_arms(Some(col), scoped_all_specialized);
                    direction_match(
                        &scoped_asc_arms,
                        &scoped_asc_fallback,
                        &scoped_desc_arms,
                        &scoped_desc_fallback,
                    )
                },
            )
        } else {
            direction_match(&asc_arms, &asc_fallback_arm, &desc_arms, &desc_fallback_arm)
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, extract_has_cursor, record_fields, record_results, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.list_for_filters_by_{}", repo_name, by_column_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, filters = tracing::field::debug(&filters), first, has_cursor, direction = tracing::field::debug(&direction), count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    let has_cursor = cursor.after.is_some();
                },
                quote! {
                    tracing::Span::current().record("first", first);
                    tracing::Span::current().record("has_cursor", has_cursor);
                },
                quote! {
                    let result_ids: Vec<_> = entities.iter().map(|e| &e.id).collect();
                    tracing::Span::current().record("count", result_ids.len());
                    tracing::Span::current().record("has_next_page", has_next_page);
                    tracing::Span::current().record("ids", tracing::field::debug(&result_ids));
                },
                quote! {
                    if let Err(ref e) = __result {
                        tracing::Span::current().record("error", true);
                        tracing::Span::current().record("exception.message", tracing::field::display(e));
                        tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                    }
                },
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, extract_has_cursor, record_fields, record_results, error_recording) =
            (quote! {}, quote! {}, quote! {}, quote! {}, quote! {});

        let post_hydrate_check = if self.post_hydrate_error.is_some() {
            quote! {
                for __entity in &entities {
                    self.execute_post_hydrate_hook(__entity).map_err(#error::PostHydrateError)?;
                }
            }
        } else {
            quote! {}
        };

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn #fn_name(
                    &self,
                    #scope_fn_arg
                    filters: #filters_ident,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                    self.#fn_in_op(#query_fn_get_op, #scope_fn_pass filters, cursor, direction).await
                }
            }
        });

        quote! {
            #standalone

            #instrument_attr
            pub async fn #fn_in_op #query_fn_generics(
                &self,
                #query_fn_op_arg,
                #scope_fn_arg
                filters: #filters_ident,
                cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                where
                    OP: #query_fn_op_traits
            {
                let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> = async {
                    #scope_convert
                    #extract_has_cursor
                    #destructure_filters
                    #destructure_tokens
                    #record_fields

                    let (entities, has_next_page) = #match_expr;

                    #post_hydrate_check
                    #record_results

                    let next_page = if has_next_page {
                        entities.last().map(#cursor_mod::#cursor_ident::from)
                    } else {
                        None
                    };
                    Ok(es_entity::PaginatedQueryRet::new(entities, next_page, first))
                }.await;

                #error_recording
                __result
            }
        }
    }
}

impl ToTokens for ListForFiltersFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let filters_name = self.filters_struct.ident();
        let sort_by_name = self.cursor.sort_by_name();
        let cursor_ident = self.cursor.ident();

        let entity = self.entity;
        let error = &self.query_error;
        let cursor_mod = &self.cursor_mod;

        let (scope_fn_arg, scope_convert) = match &self.scope {
            Some(scope) => (scope.fn_arg(), scope.convert()),
            None => (quote! {}, quote! {}),
        };

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            // Generate per-sort-column functions
            let by_fns: TokenStream = self
                .by_columns
                .iter()
                .map(|by_col| self.generate_by_fn(by_col, delete))
                .collect();

            tokens.append_all(by_fns);

            // Generate dispatch function
            let dispatch_arms = |in_op: bool| -> TokenStream {
                self
                .by_columns
                .iter()
                .map(|by_col| {
                    let by_variant = syn::Ident::new(
                        &format!("{}", by_col.name()).to_case(Case::UpperCamel),
                        Span::call_site(),
                    );
                    let inner_cursor_ident = {
                        let entity_name = format!("{}", self.entity);
                        syn::Ident::new(
                            &format!("{}_by_{}_cursor", entity_name, by_col.name())
                                .to_case(Case::UpperCamel),
                            Span::call_site(),
                        )
                    };
                    let proxy_body = self.generate_proxy_body(by_col, delete, in_op);
                    quote! {
                        #sort_by_name::#by_variant => {
                            let after = after.map(#cursor_mod::#inner_cursor_ident::try_from).transpose()?;
                            let query = es_entity::PaginatedQueryArgs { first, after };

                            #proxy_body.map_next_cursor(#cursor_mod::#cursor_ident::from)
                        }
                    }
                })
                .collect()
            };
            let dispatch_arms = dispatch_arms(true);

            let fn_name = syn::Ident::new(
                &format!("list_for_filters{}", delete.include_deletion_fn_postfix()),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_for_filters{}_in_op",
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            #[cfg(feature = "instrument")]
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = {
                let entity_name = self.entity.to_string();
                let repo_name = &self.repo_name_snake;
                let span_name = format!("{}.list_for_filters", repo_name);
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, filters = tracing::field::debug(&filters), sort_by = tracing::field::debug(&sort.by), direction = tracing::field::debug(&sort.direction), first, has_cursor, count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                    },
                    quote! {
                        let has_cursor = cursor.after.is_some();
                    },
                    quote! {
                        tracing::Span::current().record("first", first);
                        tracing::Span::current().record("has_cursor", has_cursor);
                    },
                    quote! {
                        let result_ids: Vec<_> = res.entities().iter().map(|e| &e.id).collect();
                        tracing::Span::current().record("count", result_ids.len());
                        tracing::Span::current().record("has_next_page", res.has_next_page());
                        tracing::Span::current().record("ids", tracing::field::debug(&result_ids));
                    },
                    quote! {
                        if let Err(ref e) = __result {
                            tracing::Span::current().record("error", true);
                            tracing::Span::current().record("exception.message", tracing::field::display(e));
                            tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                        }
                    },
                )
            };
            #[cfg(not(feature = "instrument"))]
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = (quote! {}, quote! {}, quote! {}, quote! {}, quote! {});

            let query_fn_generics = RepositoryOptions::query_fn_generics();
            let query_fn_op_arg = RepositoryOptions::query_fn_op_arg();
            let query_fn_op_traits = RepositoryOptions::query_fn_op_traits();
            let query_fn_get_op = RepositoryOptions::query_fn_get_op();
            let scope_fn_pass = match &self.scope {
                Some(scope) => scope.fn_pass(),
                None => quote! {},
            };

            let standalone = (!self.in_op_only).then(|| {
                quote! {
                    pub async fn #fn_name(
                        &self,
                        #scope_fn_arg
                        filters: #filters_name,
                        sort: es_entity::Sort<#sort_by_name>,
                        cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    {
                        self.#fn_in_op(#query_fn_get_op, #scope_fn_pass filters, sort, cursor).await
                    }
                }
            });

            tokens.append_all(quote! {
                #standalone

                #instrument_attr
                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    #scope_fn_arg
                    filters: #filters_name,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    where
                        OP: #query_fn_op_traits
                {
                    let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> = async {
                        #scope_convert
                        #extract_has_cursor
                        let es_entity::Sort { by, direction } = sort;
                        let es_entity::PaginatedQueryArgs { first, after } = cursor;
                        #record_fields

                        use #cursor_mod::#cursor_ident;
                        let res = match by {
                            #dispatch_arms
                        };

                        #record_results

                        Ok(res)
                    }.await;

                    #error_recording
                    __result
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    /// Build an index catalog from inline `CREATE INDEX` statements, standing in
    /// for the migrations a real repo would parse.
    fn catalog(sql: &str) -> crate::index_catalog::IndexCatalog {
        crate::index_catalog::IndexCatalog::from_sql_files(&[(
            "m.sql".to_string(),
            sql.to_string(),
        )])
    }

    #[test]
    fn filters_struct() {
        let entity = Ident::new("Order", Span::call_site());
        let customer_id_column = Column::new(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
        );
        let status_column = Column::new(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
        );

        let filters = FiltersStruct::new_test(&entity, vec![&customer_id_column, &status_column]);

        let mut tokens = TokenStream::new();
        filters.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, Default)]
            pub struct OrderFilters {
                pub customer_id: Option<CustomerId>,
                pub status: Option<OrderStatus>,
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_for_filters_function_generation() {
        let entity = Ident::new("Order", Span::call_site());
        let query_error = syn::Ident::new("OrderQueryError", Span::call_site());
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let customer_id_column = Column::new_list_for(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
            vec![id_ident.clone()],
        );
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
            vec![id_ident],
        );

        let for_columns = vec![&customer_id_column, &status_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: catalog(
                "CREATE INDEX ON orders (id); \
                 CREATE INDEX ON orders (customer_id, id); \
                 CREATE INDEX ON orders (status, id); \
                 CREATE INDEX ON orders (customer_id, status, id);",
            ),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_filters_by_id(
                &self,
                filters: OrderFilters,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrderByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderByIdCursor>, OrderQueryError> {
                self.list_for_filters_by_id_in_op(self.pool(), filters, cursor, direction).await
            }

            pub async fn list_for_filters_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                filters: OrderFilters,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrderByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderByIdCursor>, OrderQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderByIdCursor>, OrderQueryError> = async {
                    let filter_customer_id = filters.customer_id;
                    let filter_status = filters.status;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let id = if let Some(after) = after {
                        Some(after.id)
                    } else {
                        None
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => match (filter_customer_id.is_some(), filter_status.is_some(),) {
                            (false, false,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE (id > $2) AND ($2 IS NOT NULL) ORDER BY id ASC LIMIT $1) UNION ALL (SELECT id FROM orders WHERE ($2 IS NULL) ORDER BY id ASC LIMIT $1) ORDER BY id ASC LIMIT $1",
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (false, true,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE status = $1 AND (id > $3) AND ($3 IS NOT NULL) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT id FROM orders WHERE status = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2",
                                    filter_status as Option<OrderStatus>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (true, false,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE customer_id = $1 AND (id > $3) AND ($3 IS NOT NULL) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT id FROM orders WHERE customer_id = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2",
                                    filter_customer_id as Option<CustomerId>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (true, true,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE customer_id = $1 AND status = $2 AND (id > $4) AND ($4 IS NOT NULL) ORDER BY id ASC LIMIT $3) UNION ALL (SELECT id FROM orders WHERE customer_id = $1 AND status = $2 AND ($4 IS NULL) ORDER BY id ASC LIMIT $3) ORDER BY id ASC LIMIT $3",
                                    filter_customer_id as Option<CustomerId>,
                                    filter_status as Option<OrderStatus>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                        },
                        es_entity::ListDirection::Descending => match (filter_customer_id.is_some(), filter_status.is_some(),) {
                            (false, false,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE (id < $2) AND ($2 IS NOT NULL) ORDER BY id DESC LIMIT $1) UNION ALL (SELECT id FROM orders WHERE ($2 IS NULL) ORDER BY id DESC LIMIT $1) ORDER BY id DESC LIMIT $1",
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (false, true,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE status = $1 AND (id < $3) AND ($3 IS NOT NULL) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT id FROM orders WHERE status = $1 AND ($3 IS NULL) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2",
                                    filter_status as Option<OrderStatus>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (true, false,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE customer_id = $1 AND (id < $3) AND ($3 IS NOT NULL) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT id FROM orders WHERE customer_id = $1 AND ($3 IS NULL) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2",
                                    filter_customer_id as Option<CustomerId>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                            (true, true,) => {
                                es_entity::es_query!(
                                    entity = Order,
                                    "(SELECT id FROM orders WHERE customer_id = $1 AND status = $2 AND (id < $4) AND ($4 IS NOT NULL) ORDER BY id DESC LIMIT $3) UNION ALL (SELECT id FROM orders WHERE customer_id = $1 AND status = $2 AND ($4 IS NULL) ORDER BY id DESC LIMIT $3) ORDER BY id DESC LIMIT $3",
                                    filter_customer_id as Option<CustomerId>,
                                    filter_status as Option<OrderStatus>,
                                    (first + 1) as i64,
                                    id as Option<OrderId>,
                                )
                                    .fetch_n(op, first)
                                    .await?
                            },
                        }
                    };

                    let next_page = if has_next_page {
                        entities.last().map(cursor_mod::OrderByIdCursor::from)
                    } else {
                        None
                    };
                    Ok(es_entity::PaginatedQueryRet::new(entities, next_page, first))
                }.await;

                __result
            }

            pub async fn list_for_filters(
                &self,
                filters: OrderFilters,
                sort: es_entity::Sort<OrderSortBy>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrderCursor>,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderCursor>, OrderQueryError>
            {
                self.list_for_filters_in_op(self.pool(), filters, sort, cursor).await
            }

            pub async fn list_for_filters_in_op<'a, OP>(
                &self,
                op: OP,
                filters: OrderFilters,
                sort: es_entity::Sort<OrderSortBy>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrderCursor>,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderCursor>, OrderQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderCursor>, OrderQueryError> = async {
                    let es_entity::Sort { by, direction } = sort;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;

                    use cursor_mod::OrderCursor;
                    let res = match by {
                        OrderSortBy::Id => {
                            let after = after.map(cursor_mod::OrderByIdCursor::try_from).transpose()?;
                            let query = es_entity::PaginatedQueryArgs { first, after };

                            if filters.customer_id.is_none() && filters.status.is_none() {
                                self.list_by_id_in_op(op, query, direction).await?
                            } else if filters.status.is_none() {
                                self.list_for_customer_id_by_id_in_op(op, filters.customer_id.unwrap(), query, direction).await?
                            } else if filters.customer_id.is_none() {
                                self.list_for_status_by_id_in_op(op, filters.status.unwrap(), query, direction).await?
                            } else {
                                self.list_for_filters_by_id_in_op(op, filters, query, direction).await?
                            }
                            .map_next_cursor(cursor_mod::OrderCursor::from)
                        }
                    };

                    Ok(res)
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_for_filters_bare_list_for_defaults_to_by_id() {
        // Bare list_for defaults to by(id) only
        let entity = Ident::new("Order", Span::call_site());
        let query_error = syn::Ident::new("OrderQueryError", Span::call_site());
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let customer_id_column = Column::new_list_for(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
            vec![id_ident.clone()],
        );
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
            vec![id_ident],
        );

        let for_columns = vec![&customer_id_column, &status_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();

        // Bare list_for defaults to by(id), so should dispatch to individual methods for id
        assert!(token_str.contains("list_for_customer_id_by_id"));
        assert!(token_str.contains("list_for_status_by_id"));
        assert!(token_str.contains("list_for_filters_by_id"));
        assert!(token_str.contains("list_by_id"));
    }

    #[test]
    fn list_for_filters_mixed_by_columns() {
        // Test: customer_id has list_for(by(id)), status has list_for(by(created_at))
        // Only customer_id should dispatch to individual method for by_id sort
        let entity = Ident::new("Order", Span::call_site());
        let query_error = syn::Ident::new("OrderQueryError", Span::call_site());
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let created_at_ident = syn::Ident::new("created_at", proc_macro2::Span::call_site());
        // customer_id has by(id) - gets individual method for id sort
        let customer_id_column = Column::new_list_for(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
            vec![id_ident],
        );
        // status has by(created_at) - NOT paired with id sort
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
            vec![created_at_ident],
        );

        let for_columns = vec![&customer_id_column, &status_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();

        // customer_id has by(id), so dispatch should use list_for_customer_id_by_id
        assert!(token_str.contains("list_for_customer_id_by_id"));
        // status has by(created_at) not by(id), so no individual dispatch for id sort
        assert!(!token_str.contains("list_for_status_by_id"));
        // Should still have unified fallback
        assert!(token_str.contains("list_for_filters_by_id"));
    }

    #[test]
    fn list_for_filters_optional_column_uses_two_params() {
        let entity = Ident::new("Task", Span::call_site());
        let query_error = syn::Ident::new("TaskQueryError", Span::call_site());
        let id = syn::Ident::new("TaskId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("TaskId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        // Optional column: workspace_id is Option<WorkspaceId>
        let workspace_id_column = Column::new_list_for(
            syn::Ident::new("workspace_id", proc_macro2::Span::call_site()),
            syn::parse_str("Option<WorkspaceId>").unwrap(),
            vec![id_ident.clone()],
        );
        // Non-optional column: status is String
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
            vec![id_ident.clone()],
        );
        // With an empty index catalog no combination is specialized, so the
        // catch-all COALESCE fallback (the subject of this test) is emitted.
        let mk_col = |name: &str| {
            Column::new_list_for(
                syn::Ident::new(name, proc_macro2::Span::call_site()),
                syn::parse_str("String").unwrap(),
                vec![id_ident.clone()],
            )
        };
        let region_column = mk_col("region");
        let tier_column = mk_col("tier");
        let kind_column = mk_col("kind");

        let for_columns = vec![
            &workspace_id_column,
            &status_column,
            &region_column,
            &tier_column,
            &kind_column,
        ];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();

        // Optional column workspace_id uses 2 params: $1 (apply bool), $2 (value)
        // Non-optional columns use 1 param each: status $3, region $4, tier
        // $5, kind $6. So cursor params start at $7+.
        assert!(
            token_str.contains("NOT $1 OR workspace_id IS NOT DISTINCT FROM $2"),
            "Expected two-param pattern for optional column, got:\n{}",
            token_str,
        );
        assert!(
            token_str.contains("COALESCE(status = $3, $3 IS NULL)"),
            "Expected COALESCE pattern for non-optional column, got:\n{}",
            token_str,
        );

        // Verify destructuring: apply_workspace_id = is_some(), filter = flatten()
        assert!(
            token_str.contains("apply_workspace_id"),
            "Expected apply_workspace_id destructuring"
        );

        // LIMIT should be at $7 (6 filter params + 1)
        assert!(
            token_str.contains("LIMIT $7"),
            "Expected LIMIT at $7 (2 optional + 4 non-optional = 6 filter params)"
        );
    }

    #[test]
    fn list_for_filters_specializes_sargable_variants() {
        let entity = Ident::new("Task", Span::call_site());
        let query_error = syn::Ident::new("TaskQueryError", Span::call_site());
        let id = syn::Ident::new("TaskId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("TaskId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let workspace_id_column = Column::new_list_for(
            syn::Ident::new("workspace_id", proc_macro2::Span::call_site()),
            syn::parse_str("Option<WorkspaceId>").unwrap(),
            vec![id_ident.clone()],
        );
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
            vec![id_ident],
        );

        let for_columns = vec![&workspace_id_column, &status_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: catalog(
                "CREATE INDEX ON tasks (id); \
                 CREATE INDEX ON tasks (workspace_id, id); \
                 CREATE INDEX ON tasks (status, id); \
                 CREATE INDEX ON tasks (workspace_id, status, id);",
            ),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        // Each specialized combo now emits one unified `UNION ALL` query per
        // direction: an `After` branch (sargable `id > $k` gated on
        // `$k IS NOT NULL`) then the page-1 `First` branch (gated on
        // `$k IS NULL`), plus the outer merge. The filter equality predicates
        // are the leading conjuncts shared by every branch.
        let expected_queries = [
            // No filters: First branch, then After branch.
            "(SELECT id FROM tasks WHERE ($2 IS NULL) ORDER BY id ASC LIMIT $1)",
            "(SELECT id FROM tasks WHERE (id > $2) AND ($2 IS NOT NULL) ORDER BY id ASC LIMIT $1)",
            // Single non-optional filter.
            "(SELECT id FROM tasks WHERE status = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2)",
            "(SELECT id FROM tasks WHERE status = $1 AND (id > $3) AND ($3 IS NOT NULL) ORDER BY id ASC LIMIT $2)",
            // Optional filter on a value: sargable `col = $k`.
            "(SELECT id FROM tasks WHERE workspace_id = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2)",
            // Optional filter on NULL: `col IS NULL`, no parameter.
            "(SELECT id FROM tasks WHERE workspace_id IS NULL AND ($2 IS NULL) ORDER BY id ASC LIMIT $1)",
            // All filters present.
            "(SELECT id FROM tasks WHERE workspace_id = $1 AND status = $2 AND ($4 IS NULL) ORDER BY id ASC LIMIT $3)",
            "(SELECT id FROM tasks WHERE workspace_id = $1 AND status = $2 AND (id > $4) AND ($4 IS NOT NULL) ORDER BY id ASC LIMIT $3)",
            "(SELECT id FROM tasks WHERE workspace_id IS NULL AND status = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2)",
        ];
        for query in expected_queries {
            assert!(
                token_str.contains(query),
                "Expected specialized query `{query}` in generated code"
            );
        }

        // Fully-specialized entities need no wildcard fallback arm at all —
        // the explicit arms already cover the entire pattern space.
        assert!(
            !token_str.contains("COALESCE"),
            "no COALESCE fallback should be emitted when every combination is specialized"
        );
    }

    #[test]
    fn list_for_filters_specializes_equality_prefix_combos() {
        let entity = Ident::new("Wide", Span::call_site());
        let query_error = syn::Ident::new("WideQueryError", Span::call_site());
        let id = syn::Ident::new("WideId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("WideId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let mk_col = |name: &str| {
            Column::new_list_for(
                syn::Ident::new(name, proc_macro2::Span::call_site()),
                syn::parse_str("String").unwrap(),
                vec![id_ident.clone()],
            )
        };
        let col_a = mk_col("a");
        let col_b = mk_col("b");
        let col_c = mk_col("c");
        let col_d = mk_col("d");
        let col_e = mk_col("e");

        let for_columns = vec![&col_a, &col_b, &col_c, &col_d, &col_e];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns: Vec::new(),
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "wides",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            // A single composite; specialization keys on the equality columns
            // being a *leading prefix* of it — the sort column need not follow.
            index_catalog: catalog("CREATE INDEX ON wides (a, b, id);"),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        // {a}: leading prefix `a` of `(a, b, id)` → specialized.
        assert!(token_str.contains(
            "(SELECT id FROM wides WHERE a = $1 AND ($3 IS NULL) ORDER BY id ASC LIMIT $2)"
        ));
        // {a, b}: leading prefix `a, b` → specialized. The old "sort must
        // immediately follow the equality prefix" rule wrongly dropped this
        // (there is no `(a, b, id)`-vs-sort match), sending a table-growing
        // seq-scan fallback into production.
        assert!(
            token_str.contains(
                "(SELECT id FROM wides WHERE a = $1 AND b = $2 AND ($4 IS NULL) ORDER BY id ASC LIMIT $3)"
            ),
            "a combo whose equality is an index prefix must be specialized"
        );
        // {b}: `b` is not a leading prefix of `(a, b, id)` → both plans seq-scan,
        // so it correctly falls back to the COALESCE query (a build win, no
        // runtime loss).
        assert!(
            !token_str.contains("(SELECT id FROM wides WHERE b = $1 AND ($3 IS NULL)"),
            "`b` alone is not a leading index prefix and must fall back"
        );
        assert!(
            token_str.contains("COALESCE(b = $2, $2 IS NULL)"),
            "COALESCE fallback must remain for combos with no index prefix"
        );
    }

    /// With an empty index catalog (a repo with no declared indexes, or no
    /// migrations dir) the multi-filter cartesian matrix is suppressed: only
    /// the catch-all COALESCE query is emitted, regardless of filter-column
    /// count. This is what keeps downstream compile times bounded (the fix for
    /// the lana-bank +2054-query regression).
    #[test]
    fn list_for_filters_no_index_emits_catch_all_only() {
        let entity = Ident::new("Order", Span::call_site());
        let query_error = syn::Ident::new("OrderQueryError", Span::call_site());
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let customer_id_column = Column::new_list_for(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
            vec![id_ident.clone()],
        );
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
            vec![id_ident],
        );

        let for_columns = vec![&customer_id_column, &status_column];
        let by_columns = vec![&id_column];
        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };
        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let build = |index_catalog: crate::index_catalog::IndexCatalog| -> String {
            let fn_ = ListForFiltersFn {
                in_op_only: false,
                filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
                entity: &entity,
                query_error: query_error.clone(),
                for_columns: for_columns.clone(),
                virtual_columns: Vec::new(),
                by_columns: by_columns.clone(),
                cursor: &combo_cursor,
                delete: DeleteOption::No,
                cursor_mod: cursor_mod.clone(),
                table_name: "orders",
                ignore_prefix: None,
                id: &id,
                post_hydrate_error: None,
                forgettable_table_name: None,
                snapshot_table_name: None,
                scope: None,
                index_catalog,
                #[cfg(feature = "instrument")]
                repo_name_snake: "test_repo".to_string(),
            };
            let mut tokens = TokenStream::new();
            fn_.to_tokens(&mut tokens);
            tokens.to_string()
        };

        let off = build(Default::default());
        let on = build(catalog(
            "CREATE INDEX ON orders (id); \
             CREATE INDEX ON orders (customer_id, id); \
             CREATE INDEX ON orders (status, id); \
             CREATE INDEX ON orders (customer_id, status, id);",
        ));

        let count = |s: &str| s.matches("es_query").count();
        let off_count = count(&off);
        let on_count = count(&on);

        // An index-backed catalog specializes all 2^2 = 4 filter combos (no
        // COALESCE); an empty catalog collapses to a single catch-all COALESCE
        // query.
        assert!(
            on_count > off_count,
            "indexed catalog should emit more dedicated queries: on={on_count} off={off_count}"
        );
        assert!(
            off.contains("COALESCE"),
            "empty catalog must emit the catch-all COALESCE query"
        );
        assert!(
            !on.contains("COALESCE"),
            "fully-indexed catalog should not need the COALESCE fallback"
        );
    }

    /// A virtual column alongside a physical `list_for` column: the filters
    /// struct gains an `Option<bool>` field, the "no filters" and
    /// single-physical-filter dispatch checks both require it to be absent,
    /// and — with no index catalog to specialize anything — the fallback
    /// query carries the predicate as a static `AND (EXISTS (...))`
    /// conjunct with no bind parameter of its own.
    #[test]
    fn virtual_filter_column_generates_option_bool_field_and_none_checks() {
        let entity = Ident::new("Task", Span::call_site());
        let query_error = syn::Ident::new("TaskQueryError", Span::call_site());
        let id = syn::Ident::new("TaskId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("TaskId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
            vec![id_ident],
        );
        let flagged_column = Column::new_virtual(
            syn::Ident::new("flagged", proc_macro2::Span::call_site()),
            "EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)",
        );

        let for_columns = vec![&status_column];
        let virtual_columns = vec![&flagged_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };
        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test_with_virtual(
                &entity,
                for_columns.clone(),
                virtual_columns.clone(),
            ),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut struct_tokens = TokenStream::new();
        list_for_filters_fn
            .filters_struct
            .to_tokens(&mut struct_tokens);
        let struct_str = struct_tokens.to_string();

        // The filters struct carries the virtual column as `Option<bool>`,
        // after the physical fields.
        assert!(
            struct_str.contains("pub status : Option < String > , pub flagged : Option < bool > ,"),
            "unexpected filters struct fields:\n{struct_str}"
        );

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        // The "no filters at all" proxy check requires the virtual field to
        // be absent too.
        assert!(
            token_str.contains("filters . status . is_none () && filters . flagged . is_none ()"),
            "expected the virtual column in the all-none dispatch check:\n{token_str}"
        );
        // `status` alone has a dedicated `list_for_status_by_id` fn, but a
        // virtual `Some` must never take that shortcut.
        assert!(
            token_str.contains("filters . flagged . is_none ()"),
            "expected a virtual none-check gating the single-column shortcut:\n{token_str}"
        );

        // With no index catalog, the unified fallback query carries the
        // predicate as an unparameterized static conjunct.
        assert!(
            token_str
                .contains("AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id))"),
            "expected the virtual predicate as a static AND conjunct:\n{token_str}"
        );
        // It binds no query parameter of its own — every bind in the
        // fallback query is still `filter_status` (the only physical
        // for_column) or a cursor argument.
        assert!(
            !token_str.contains("flagged as"),
            "the virtual predicate must not bind a query parameter:\n{token_str}"
        );
    }

    /// Regression guard for the compatibility contract of the parameterized
    /// (value) virtual columns change: a repo with only a bool-polarity
    /// virtual column (the #236 shape — no value virtuals) must expand to
    /// byte-identical tokens before and after this change. The expected
    /// string below was captured from the same fixture as
    /// [`virtual_filter_column_generates_option_bool_field_and_none_checks`]
    /// running against the pre-change (#236) codegen, and diffed
    /// byte-for-byte against the post-change output before being pasted
    /// here — this is not a re-derivation, it is that captured baseline.
    #[test]
    fn virtual_filter_column_bool_only_output_is_byte_identical_to_pre_value_virtual_baseline() {
        let entity = Ident::new("Task", Span::call_site());
        let query_error = syn::Ident::new("TaskQueryError", Span::call_site());
        let id = syn::Ident::new("TaskId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("TaskId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
            vec![id_ident],
        );
        let flagged_column = Column::new_virtual(
            syn::Ident::new("flagged", proc_macro2::Span::call_site()),
            "EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)",
        );

        let for_columns = vec![&status_column];
        let virtual_columns = vec![&flagged_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };
        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test_with_virtual(
                &entity,
                for_columns.clone(),
                virtual_columns.clone(),
            ),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        let expected = r#"pub async fn list_for_filters_by_id (& self , filters : TaskFilters , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: TaskByIdCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskByIdCursor > , TaskQueryError > { self . list_for_filters_by_id_in_op (self . pool () , filters , cursor , direction) . await } pub async fn list_for_filters_by_id_in_op < 'a , OP > (& self , op : OP , filters : TaskFilters , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: TaskByIdCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskByIdCursor > , TaskQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskByIdCursor > , TaskQueryError > = async { let filter_status = filters . status ; let virtual_flagged = filters . flagged ; let es_entity :: PaginatedQueryArgs { first , after } = cursor ; let id = if let Some (after) = after { Some (after . id) } else { None } ; let (entities , has_next_page) = match direction { es_entity :: ListDirection :: Ascending => match (filter_status . is_some () , virtual_flagged ,) { (.. , None ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id > $3) AND ($3 IS NOT NULL) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , (.. , Some (true) ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id > $3) AND ($3 IS NOT NULL) AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , (.. , Some (false) ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id > $3) AND ($3 IS NOT NULL) AND NOT (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) AND NOT (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , } , es_entity :: ListDirection :: Descending => match (filter_status . is_some () , virtual_flagged ,) { (.. , None ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id < $3) AND ($3 IS NOT NULL) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , (.. , Some (true) ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id < $3) AND ($3 IS NOT NULL) AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , (.. , Some (false) ,) => es_entity :: es_query ! (entity = Task , "(SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND (id < $3) AND ($3 IS NOT NULL) AND NOT (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT id FROM tasks WHERE COALESCE(status = $1, $1 IS NULL) AND ($3 IS NULL) AND NOT (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2" , filter_status as Option < String > , (first + 1) as i64 , id as Option < TaskId > ,) . fetch_n (op , first) . await ? , } } ; let end_cursor = entities . last () . map (cursor_mod :: TaskByIdCursor :: from) ; Ok (es_entity :: PaginatedQueryRet :: new (entities , has_next_page , end_cursor , first)) } . await ; __result } pub async fn list_for_filters (& self , filters : TaskFilters , sort : es_entity :: Sort < TaskSortBy > , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: TaskCursor > ,) -> Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskCursor > , TaskQueryError > { self . list_for_filters_in_op (self . pool () , filters , sort , cursor) . await } pub async fn list_for_filters_in_op < 'a , OP > (& self , op : OP , filters : TaskFilters , sort : es_entity :: Sort < TaskSortBy > , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: TaskCursor > ,) -> Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskCursor > , TaskQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Task , cursor_mod :: TaskCursor > , TaskQueryError > = async { let es_entity :: Sort { by , direction } = sort ; let es_entity :: PaginatedQueryArgs { first , after } = cursor ; use cursor_mod :: TaskCursor ; let res = match by { TaskSortBy :: Id => { let after = after . map (cursor_mod :: TaskByIdCursor :: try_from) . transpose () ? ; let query = es_entity :: PaginatedQueryArgs { first , after } ; if filters . status . is_none () && filters . flagged . is_none () { self . list_by_id_in_op (op , query , direction) . await ? } else if filters . flagged . is_none () { self . list_for_status_by_id_in_op (op , filters . status . unwrap () , query , direction) . await ? } else { self . list_for_filters_by_id_in_op (op , filters , query , direction) . await ? } . map_end_cursor (cursor_mod :: TaskCursor :: from) } } ; Ok (res) } . await ; __result }"#;

        assert_eq!(
            token_str, expected,
            "bool-only virtual codegen changed — this must stay byte-identical to the pre-value-virtual (#236) baseline"
        );
    }

    /// A repo with one physical `list_for` column, one bool-polarity virtual,
    /// and one *value* virtual: the filters struct gains all three fields in
    /// declaration order, the two virtual dimensions compose (the `Present`
    /// value-virtual arm binds its `$k` right after the physical filter
    /// param and before `(first + 1) as i64`), and the bool virtual's
    /// unparameterized `(EXISTS (...))` conjunct sits alongside the value
    /// virtual's bound `>= $k` conjunct in the same query. Also closes the
    /// "more than one virtual column" case, previously untested.
    #[test]
    fn mixed_physical_bool_virtual_and_value_virtual_columns_compose() {
        let entity = Ident::new("Order", Span::call_site());
        let query_error = syn::Ident::new("OrderQueryError", Span::call_site());
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let id_ident = syn::Ident::new("id", proc_macro2::Span::call_site());
        let status_column = Column::new_list_for(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
            vec![id_ident],
        );
        let flagged_column = Column::new_virtual(
            syn::Ident::new("flagged", proc_macro2::Span::call_site()),
            "EXISTS (SELECT 1 FROM order_flags o WHERE o.order_id = orders.id)",
        );
        let min_flags_column = Column::new_virtual_value(
            syn::Ident::new("min_flags", proc_macro2::Span::call_site()),
            syn::parse_str("i64").unwrap(),
            "(SELECT COUNT(*) FROM order_flags o WHERE o.order_id = orders.id) >= {value}",
        );

        let for_columns = vec![&status_column];
        let virtual_columns = vec![&flagged_column, &min_flags_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };
        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test_with_virtual(
                &entity,
                for_columns.clone(),
                virtual_columns.clone(),
            ),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut struct_tokens = TokenStream::new();
        list_for_filters_fn
            .filters_struct
            .to_tokens(&mut struct_tokens);
        let struct_str = struct_tokens.to_string();
        assert!(
            struct_str.contains(
                "pub status : Option < String > , pub flagged : Option < bool > , pub min_flags : Option < i64 > ,"
            ),
            "unexpected filters struct fields:\n{struct_str}"
        );

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();

        // No index catalog: every combo falls back to the COALESCE query,
        // with the physical filter bound at $1 (the sole physical for_column
        // is non-optional, so it consumes exactly one param).
        //
        // The bool virtual's `True` state contributes an unparameterized
        // `(EXISTS (...))` conjunct; the value virtual's `Present` state
        // contributes its substituted predicate with `{value}` rewritten to
        // `$2` (the next param after the physical filter's `$1`) — both in
        // the same query, proving the two virtual dimensions compose.
        assert!(
            token_str.contains(
                "AND (EXISTS (SELECT 1 FROM order_flags o WHERE o.order_id = orders.id)) AND ((SELECT COUNT(*) FROM order_flags o WHERE o.order_id = orders.id) >= $2)"
            ),
            "expected both virtual conjuncts (bool + value) in the same fallback query:\n{token_str}"
        );

        // The value virtual's bound arg (`filter_min_flags`) follows the
        // physical arg (`filter_status`) and precedes the limit arg — the
        // allocation order `substitute_value_placeholder` and
        // `build_fallback` are documented to produce.
        assert!(
            token_str.contains(
                "filter_status as Option < String > , filter_min_flags as Option < i64 > , (first + 1) as i64"
            ),
            "expected filter_min_flags bound after the physical arg and before the limit arg:\n{token_str}"
        );
    }

    /// A repo with *only* a virtual filter column (no physical `list_for`
    /// columns) still emits the full `list_for_filters*` apparatus, and the
    /// dispatch proxy routes through it whenever the virtual filter is set
    /// — it must never silently degrade to plain `list_by`.
    #[test]
    fn virtual_only_filter_column_still_dispatches_through_list_for_filters() {
        let entity = Ident::new("Task", Span::call_site());
        let query_error = syn::Ident::new("TaskQueryError", Span::call_site());
        let id = syn::Ident::new("TaskId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("TaskId").unwrap());
        let flagged_column = Column::new_virtual(
            syn::Ident::new("flagged", proc_macro2::Span::call_site()),
            "EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id)",
        );

        let for_columns: Vec<&Column> = vec![];
        let virtual_columns = vec![&flagged_column];
        let by_columns = vec![&id_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };
        let combo_cursor = ComboCursor::new_test(&entity, vec![id_cursor]);

        let list_for_filters_fn = ListForFiltersFn {
            in_op_only: false,
            filters_struct: FiltersStruct::new_test_with_virtual(
                &entity,
                for_columns.clone(),
                virtual_columns.clone(),
            ),
            entity: &entity,
            query_error,
            for_columns,
            virtual_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
            snapshot_table_name: None,
            scope: None,
            index_catalog: Default::default(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut struct_tokens = TokenStream::new();
        list_for_filters_fn
            .filters_struct
            .to_tokens(&mut struct_tokens);
        let struct_str = struct_tokens.to_string();
        assert!(struct_str.contains("pub struct TaskFilters"));
        assert!(struct_str.contains("pub flagged : Option < bool > ,"));

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);
        let token_str = tokens.to_string();
        // The proxy must check the virtual filter before falling back to
        // plain `list_by`, and route to `list_for_filters_by_id` (never a
        // per-column fn — none exists for a virtual column) when it's set.
        assert!(
            token_str.contains(
                "if filters . flagged . is_none () { self . list_by_id_in_op (op , query , direction) . await ? }"
            ),
            "unexpected proxy body:\n{token_str}"
        );
        assert!(
            token_str.contains(
                "self . list_for_filters_by_id_in_op (op , filters , query , direction) . await ?"
            ),
            "expected the virtual-only case to fall back to the unified filters fn:\n{token_str}"
        );
        assert!(
            token_str
                .contains("AND (EXISTS (SELECT 1 FROM task_flags f WHERE f.task_id = tasks.id))"),
            "expected the virtual predicate in the fallback query:\n{token_str}"
        );
    }

    /// `virtual = "..."` is rejected outright when it isn't paired with a
    /// bare `list_for` — asserted at the full-derive level (rather than
    /// [`super::super::options::columns`]'s narrower unit tests) so the
    /// error surfaces exactly the way a consumer would see it.
    #[test]
    fn virtual_column_without_list_for_is_rejected_by_derive() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[es_repo(
                entity = "Task",
                columns(flagged(ty = "bool", virtual = "TRUE"))
            )]
            struct Tasks {
                pool: sqlx::PgPool,
            }
        };
        let err = super::super::derive(input).unwrap_err();
        assert!(
            err.to_string().contains("must be marked `list_for`"),
            "unexpected error: {err}"
        );
    }
}
