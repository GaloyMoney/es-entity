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

pub struct FiltersStruct<'a> {
    columns: Vec<&'a Column>,
    entity: &'a syn::Ident,
}

impl<'a> FiltersStruct<'a> {
    pub fn new(opts: &'a RepositoryOptions, columns: Vec<&'a Column>) -> Self {
        Self {
            entity: opts.entity(),
            columns,
        }
    }

    #[cfg(test)]
    fn new_test(entity: &'a syn::Ident, columns: Vec<&'a Column>) -> Self {
        Self { entity, columns }
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
    pub filters_struct: FiltersStruct<'a>,
    entity: &'a syn::Ident,
    query_error: syn::Ident,
    for_columns: Vec<&'a Column>,
    by_columns: Vec<&'a Column>,
    cursor: &'a ComboCursor<'a>,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    table_name: &'a str,
    ignore_prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    post_hydrate_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    scope: Option<ScopeInfo<'a>>,
    index_catalog: crate::index_catalog::IndexCatalog,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListForFiltersFn<'a> {
    pub fn new(
        opts: &'a RepositoryOptions,
        for_columns: Vec<&'a Column>,
        by_columns: Vec<&'a Column>,
        cursor: &'a ComboCursor<'a>,
    ) -> Self {
        Self {
            filters_struct: FiltersStruct::new(opts, for_columns.clone()),
            entity: opts.entity(),
            query_error: opts.query_error(),
            for_columns,
            by_columns,
            cursor,
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            table_name: opts.table_name(),
            ignore_prefix: opts.table_prefix(),
            id: opts.id(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            scope: ScopeInfo::from_opts(opts),
            index_catalog: opts.index_catalog(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
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

                tokens.append_all(quote! {
                    pub async fn #fn_name(
                        &self,
                        filters: #filters_ident,
                        cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                        direction: es_entity::ListDirection,
                    ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                        self.repo.#fn_name(self.scope, filters, cursor, direction).await
                    }

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
                &format!("list_for_filters{}", delete_postfix),
                Span::call_site(),
            );
            tokens.append_all(quote! {
                pub async fn #dispatch_fn(
                    &self,
                    filters: #filters_ident,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#combo_cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#combo_cursor_ident>, #error> {
                    self.repo.#dispatch_fn(self.scope, filters, sort, cursor).await
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
        tokens
    }

    /// Scrutinee elements (bools over the destructured filter locals)
    /// identifying each filter's [`FilterState`] at runtime: one bool per
    /// non-optional column (`is_some`), two per optional column (`apply`,
    /// value `is_some`).
    fn filter_scrutinee_elems(&self) -> Vec<TokenStream> {
        self.for_columns
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
            .collect()
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

    fn generate_proxy_body(&self, by_col: &Column, delete: DeleteOption) -> TokenStream {
        let by_col_name = by_col.name();
        let delete_postfix = delete.include_deletion_fn_postfix();

        let scope_pass = if self.scope.is_some() {
            quote! { __scope, }
        } else {
            quote! {}
        };

        let list_by_fn = syn::Ident::new(
            &format!("list_by_{}{}", by_col_name, delete_postfix),
            Span::call_site(),
        );

        if self.for_columns.is_empty() {
            return quote! { self.#list_by_fn(#scope_pass query, direction).await? };
        }

        let all_none_checks: Vec<_> = self
            .for_columns
            .iter()
            .map(|c| {
                let name = c.name();
                quote! { filters.#name.is_none() }
            })
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
                let others_none: Vec<_> = self
                    .for_columns
                    .iter()
                    .filter(|c| c.name() != for_col.name())
                    .map(|c| {
                        let name = c.name();
                        quote! { filters.#name.is_none() }
                    })
                    .collect();

                let for_col_name = for_col.name();
                let fn_name = syn::Ident::new(
                    &format!(
                        "list_for_{}_by_{}{}",
                        for_col_name, by_col_name, delete_postfix
                    ),
                    Span::call_site(),
                );

                if others_none.is_empty() {
                    quote! {
                        else {
                            self.#fn_name(#scope_pass filters.#for_col_name.unwrap(), query, direction).await?
                        }
                    }
                } else {
                    quote! {
                        else if #(#others_none)&&* {
                            self.#fn_name(#scope_pass filters.#for_col_name.unwrap(), query, direction).await?
                        }
                    }
                }
            })
            .collect();

        // Need a fallback when:
        // - there are unpaired for_columns (they need COALESCE)
        // - there are 2+ paired columns (multi-filter case)
        // - there are 2+ for_columns total (multi-filter case)
        let has_unpaired = paired_for_columns.len() < self.for_columns.len();
        let needs_fallback = has_unpaired || self.for_columns.len() >= 2;
        let multi_filter_fallback = if needs_fallback {
            let list_for_filters_fn = syn::Ident::new(
                &format!("list_for_filters_by_{}{}", by_col_name, delete_postfix),
                Span::call_site(),
            );
            quote! {
                else {
                    self.#list_for_filters_fn(#scope_pass filters, query, direction).await?
                }
            }
        } else {
            quote! {}
        };

        quote! {
            if #(#all_none_checks)&&* {
                self.#list_by_fn(#scope_pass query, direction).await?
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
            .collect();

        // Generate the non-specialized fallback query: correct for every filter
        // combination, sargable only where a matching composite index exists.
        // The filter predicates (COALESCE / apply-flag forms) are the leading
        // conjuncts shared by every unified-cursor `UNION ALL` branch.
        // Parameterized over the scope: each scope-column arm binds its
        // column at `$1` and shifts every other parameter by one.
        let build_fallback = |scope: Option<&ScopeCol>| -> (String, String, TokenStream) {
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

            let filter_arg_bindings: TokenStream = self
                .for_columns
                .iter()
                .map(|col| FiltersStruct::filter_arg_tokens(col))
                .collect();
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
        let (asc_query, desc_query, fallback_arg_tokens) = build_fallback(None);

        let forgettable_tbl_arg = if let Some(tbl) = self.forgettable_table_name {
            quote! { forgettable_tbl = #tbl, }
        } else {
            quote! {}
        };

        let make_es_query = |query: &str, args: &TokenStream| -> TokenStream {
            if let Some(prefix) = self.ignore_prefix {
                quote! {
                    es_entity::es_query!(
                        tbl_prefix = #prefix,
                        #forgettable_tbl_arg
                        #query,
                        #args
                    )
                }
            } else {
                quote! {
                    es_entity::es_query!(
                        entity = #entity,
                        #forgettable_tbl_arg
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
        let build_specialized_arms =
            |scope: Option<&ScopeCol>| -> (TokenStream, TokenStream, bool) {
                let mut asc_arms = TokenStream::new();
                let mut desc_arms = TokenStream::new();
                let mut all_combos_specialized = true;
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
                                filter_conditions.push(format!("{} = ${}", col.name(), param_idx));
                                param_idx += 1;
                                filter_args.append_all(FiltersStruct::filter_arg_tokens(col));
                            }
                            FilterState::PresentNull => {
                                filter_conditions.push(format!("{} IS NULL", col.name()));
                            }
                            FilterState::PresentValue => {
                                filter_conditions.push(format!("{} = ${}", col.name(), param_idx));
                                param_idx += 1;
                                filter_args.append_all(FiltersStruct::filter_value_arg_tokens(col));
                            }
                        }
                    }

                    // The cursor-state dimension collapses into one unified
                    // `UNION ALL` query per direction (the specialized filter
                    // predicates are the leading conjuncts of every branch), so
                    // the arm matches on the filter scrutinee alone.
                    let pattern = quote! { (#(#filter_patterns,)*) };
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
                (asc_arms, desc_arms, all_combos_specialized)
            };
        let (asc_arms, desc_arms, all_combos_specialized) = build_specialized_arms(None);

        let scrutinee_elems: Vec<TokenStream> = self.filter_scrutinee_elems();

        // When every filter combination is specialized the explicit arms
        // already cover the entire pattern space, so no wildcard fallback arm
        // (nor its catch-all COALESCE queries) is emitted.
        let build_fallback_arms = |asc_query: &str,
                                   desc_query: &str,
                                   args: &TokenStream,
                                   all_specialized: bool|
         -> (TokenStream, TokenStream) {
            if all_specialized {
                (quote! {}, quote! {})
            } else {
                let asc_call = make_es_query(asc_query, args);
                let desc_call = make_es_query(desc_query, args);
                (
                    quote! { _ => #asc_call.fetch_n(op, first).await?, },
                    quote! { _ => #desc_call.fetch_n(op, first).await?, },
                )
            }
        };
        let (asc_fallback_arm, desc_fallback_arm) = build_fallback_arms(
            &asc_query,
            &desc_query,
            &fallback_arg_tokens,
            all_combos_specialized,
        );

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
                    let (scoped_asc_query, scoped_desc_query, scoped_fallback_args) =
                        build_fallback(Some(col));
                    let (scoped_asc_fallback, scoped_desc_fallback) = build_fallback_arms(
                        &scoped_asc_query,
                        &scoped_desc_query,
                        &scoped_fallback_args,
                        scoped_all_specialized,
                    );
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

                    let end_cursor = entities.last().map(#cursor_mod::#cursor_ident::from);

                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
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
            let dispatch_arms: TokenStream = self
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
                    let proxy_body = self.generate_proxy_body(by_col, delete);
                    quote! {
                        #sort_by_name::#by_variant => {
                            let after = after.map(#cursor_mod::#inner_cursor_ident::try_from).transpose()?;
                            let query = es_entity::PaginatedQueryArgs { first, after };

                            let es_entity::PaginatedQueryRet {
                                entities,
                                has_next_page,
                                end_cursor,
                            } = #proxy_body;
                            es_entity::PaginatedQueryRet {
                                entities,
                                has_next_page,
                                end_cursor: end_cursor.map(#cursor_mod::#cursor_ident::from)
                            }
                        }
                    }
                })
                .collect();

            let fn_name = syn::Ident::new(
                &format!("list_for_filters{}", delete.include_deletion_fn_postfix()),
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
                        let result_ids: Vec<_> = res.entities.iter().map(|e| &e.id).collect();
                        tracing::Span::current().record("count", result_ids.len());
                        tracing::Span::current().record("has_next_page", res.has_next_page);
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

            tokens.append_all(quote! {
                #instrument_attr
                pub async fn #fn_name(
                    &self,
                    #scope_fn_arg
                    filters: #filters_name,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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

                    let end_cursor = entities.last().map(cursor_mod::OrderByIdCursor::from);

                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
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
                let __result: Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrderCursor>, OrderQueryError> = async {
                    let es_entity::Sort { by, direction } = sort;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;

                    use cursor_mod::OrderCursor;
                    let res = match by {
                        OrderSortBy::Id => {
                            let after = after.map(cursor_mod::OrderByIdCursor::try_from).transpose()?;
                            let query = es_entity::PaginatedQueryArgs { first, after };

                            let es_entity::PaginatedQueryRet {
                                entities,
                                has_next_page,
                                end_cursor,
                            } = if filters.customer_id.is_none() && filters.status.is_none() {
                                self.list_by_id(query, direction).await?
                            } else if filters.status.is_none() {
                                self.list_for_customer_id_by_id(filters.customer_id.unwrap(), query, direction).await?
                            } else if filters.customer_id.is_none() {
                                self.list_for_status_by_id(filters.status.unwrap(), query, direction).await?
                            } else {
                                self.list_for_filters_by_id(filters, query, direction).await?
                            };
                            es_entity::PaginatedQueryRet {
                                entities,
                                has_next_page,
                                end_cursor: end_cursor.map(cursor_mod::OrderCursor::from)
                            }
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "tasks",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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
            filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
            entity: &entity,
            query_error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "wides",
            ignore_prefix: None,
            id: &id,
            post_hydrate_error: None,
            forgettable_table_name: None,
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
                filters_struct: FiltersStruct::new_test(&entity, for_columns.clone()),
                entity: &entity,
                query_error: query_error.clone(),
                for_columns: for_columns.clone(),
                by_columns: by_columns.clone(),
                cursor: &combo_cursor,
                delete: DeleteOption::No,
                cursor_mod: cursor_mod.clone(),
                table_name: "orders",
                ignore_prefix: None,
                id: &id,
                post_hydrate_error: None,
                forgettable_table_name: None,
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
}
