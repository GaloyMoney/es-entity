use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{
    options::*,
    scope::{ScopeCol, ScopeInfo},
};

/// Assemble the unified cursor query: the per-state cursor predicates emitted
/// as `UNION ALL` branches in a single static query.
///
/// Each branch carries its own `ORDER BY ... LIMIT` — this is load-bearing:
/// without the per-branch order+limit the planner falls back to a full-table
/// top-N sort instead of an early-exit index walk (`Merge Append` of ordered
/// index scans). The outer `ORDER BY ... LIMIT` then merges the branches.
///
/// Every branch is gated by a parameter-only nullness guard (see
/// [`CursorStruct::cursor_branches`]) that Postgres lifts into a `One-Time
/// Filter`, so dead branches are skipped at execution and the live branch gets
/// a plan identical to its standalone per-state twin. `leading` conjuncts
/// (scope, filter) and `trailing` conjuncts (soft-delete) are replicated
/// inside every branch.
///
/// Within each branch the cursor `predicate` is emitted **before** the gate,
/// and [`CursorStruct::cursor_branches`] orders the branches so the page-1
/// branch (whose only reference to the cursor `id` parameter is the bare
/// `$id IS NULL` guard) comes last. This guarantees every parameter's first
/// textual occurrence in the statement is a typed column comparison — required
/// because Postgres infers untyped prepared-statement parameter types from
/// their leftmost use across a `UNION`, and a bare `$n IS NULL` there yields
/// "could not determine data type of parameter". Ordering conjuncts is free
/// for both correctness (all `AND`ed) and the plan (the One-Time Filter is
/// still recognised regardless of conjunct order — verified via EXPLAIN).
pub fn assemble_union_select(
    select_columns: &str,
    table_name: &str,
    leading: &[String],
    branches: &[(String, Option<String>)],
    trailing: &[String],
    order_by: &str,
    limit_param_idx: u32,
) -> String {
    let branch_sqls: Vec<String> = branches
        .iter()
        .map(|(gate, predicate)| {
            let mut conditions: Vec<String> = leading.to_vec();
            if let Some(predicate) = predicate {
                conditions.push(format!("({predicate})"));
            }
            conditions.push(format!("({gate})"));
            conditions.extend(trailing.iter().cloned());
            format!(
                "(SELECT {select_columns} FROM {table_name} WHERE {} ORDER BY {order_by} LIMIT ${limit_param_idx})",
                conditions.join(" AND "),
            )
        })
        .collect();

    format!(
        "{} ORDER BY {order_by} LIMIT ${limit_param_idx}",
        branch_sqls.join(" UNION ALL "),
    )
}

/// The `deleted = FALSE` predicate for repos with soft delete, on the
/// non-`include_deleted` fn variants.
pub fn not_deleted_predicate(delete: DeleteOption) -> Option<String> {
    if delete.is_soft() {
        Some("deleted = FALSE".to_string())
    } else {
        None
    }
}

pub struct CursorStruct<'a> {
    pub id: &'a syn::Ident,
    pub entity: &'a syn::Ident,
    pub column: &'a Column,
    pub cursor_mod: &'a syn::Ident,
}

impl CursorStruct<'_> {
    fn name(&self) -> String {
        let entity_name = format!("{}", self.entity);
        format!("{}_by_{}_cursor", entity_name, self.column.name()).to_case(Case::UpperCamel)
    }

    pub fn ident(&self) -> syn::Ident {
        syn::Ident::new(&self.name(), Span::call_site())
    }

    pub fn cursor_mod(&self) -> &syn::Ident {
        self.cursor_mod
    }

    pub fn select_columns(&self, for_column: Option<&syn::Ident>) -> String {
        let mut for_column_str = String::new();
        if let Some(for_column) = for_column
            && self.column.name() != for_column
        {
            for_column_str = format!("{for_column}, ");
        }
        if self.column.is_id() {
            format!("{for_column_str}id")
        } else {
            format!("{}{}, id", for_column_str, self.column.name())
        }
    }

    pub fn order_by(&self, ascending: bool) -> String {
        let dir = if ascending { "ASC" } else { "DESC" };
        let nulls = if ascending { "FIRST" } else { "LAST" };
        if self.column.is_id() {
            format!("id {dir}")
        } else if self.column.is_nullable_column() {
            format!("{0} {dir} NULLS {nulls}, id {dir}", self.column.name())
        } else {
            format!("{} {dir}, id {dir}", self.column.name())
        }
    }

    /// The `UNION ALL` branches of the unified cursor query for one direction.
    ///
    /// Each branch is a `(gate, predicate)` pair. `gate` is a parameter-only
    /// nullness guard — it references only `$id`/`$col`, never a row column —
    /// so Postgres lifts it into a `One-Time Filter` Result node and skips dead
    /// branches at execution; the single live branch then gets a plan identical
    /// to its standalone per-state twin. `predicate` is the sargable cursor
    /// comparison (`None` for the page-1 branch, which needs no predicate).
    ///
    /// - id / non-nullable sort column: 2 branches (First, After).
    /// - `Option<T>` or `nullable`-annotated sort column: 3 branches (First,
    ///   After, AfterNull).
    ///
    /// The `nullable`-annotated (non-`Option`) case shares the same 3-branch
    /// form as `Option<T>`: the gates dispatch on `$col IS NULL` *in the
    /// database*, where the encoded SQL NULL-ness is always visible — even
    /// though Rust cannot see it from the non-`Option` type. This is what lets
    /// these columns use the sargable branch form instead of the non-sargable
    /// `COALESCE` catch-all (formerly `AfterMaybeNull`), making them
    /// index-friendly for the first time.
    ///
    /// Branches are ordered so the page-1 (First) branch — whose only cursor
    /// reference is the bare `$id IS NULL` guard — comes **last**. Every other
    /// branch leads with a typed column comparison of the cursor parameters, so
    /// each parameter's first textual occurrence in the assembled `UNION` is
    /// type-determined. See [`assemble_union_select`] for why this matters.
    pub fn cursor_branches(&self, offset: u32, ascending: bool) -> Vec<(String, Option<String>)> {
        let comp = if ascending { ">" } else { "<" };
        let id_offset = offset + 2;
        let column_offset = offset + 3;

        let first = (format!("${id_offset} IS NULL"), None);

        // id column, or non-nullable sort column: 2 branches (After, First).
        if self.column.is_id() {
            let after = (
                format!("${id_offset} IS NOT NULL"),
                Some(format!("id {comp} ${id_offset}")),
            );
            return vec![after, first];
        }
        if !self.column.is_nullable_column() {
            let after = (
                format!("${id_offset} IS NOT NULL"),
                Some(format!(
                    "({0}, id) {comp} (${column_offset}, ${id_offset})",
                    self.column.name()
                )),
            );
            return vec![after, first];
        }

        // `Option<T>` or `nullable`-annotated column: 3 branches
        // (After, AfterNull, First). Predicates mirror the direction-aware
        // NULL edge semantics; the AfterNull gate dispatches on `$col IS NULL`
        // in the database.
        let after_predicate = if ascending {
            format!(
                "({0}, id) > (${column_offset}, ${id_offset})",
                self.column.name()
            )
        } else {
            format!(
                "({0} IS NULL OR ({0}, id) < (${column_offset}, ${id_offset}))",
                self.column.name()
            )
        };
        let after_null_predicate = if ascending {
            format!("({0} IS NOT NULL OR id > ${id_offset})", self.column.name())
        } else {
            format!("({0} IS NULL AND id < ${id_offset})", self.column.name())
        };
        let after = (
            format!("${id_offset} IS NOT NULL AND ${column_offset} IS NOT NULL"),
            Some(after_predicate),
        );
        let after_null = (
            format!("${id_offset} IS NOT NULL AND ${column_offset} IS NULL"),
            Some(after_null_predicate),
        );
        vec![after, after_null, first]
    }

    /// The full cursor-value bindings (`id`, and the sort column for
    /// non-id cursors), always in the same shape regardless of cursor state —
    /// the unified query binds every parameter once and the branch gates
    /// dispatch on their nullness in SQL.
    pub fn cursor_arg_tokens(&self) -> TokenStream {
        let id = self.id;

        if self.column.is_id() {
            quote! {
                id as Option<#id>,
            }
        } else if self.column.is_optional() {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            quote! {
                id as Option<#id>,
                #column_name as #column_type,
            }
        } else {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            quote! {
                id as Option<#id>,
                #column_name as Option<#column_type>,
            }
        }
    }

    pub fn query_arg_tokens(&self) -> TokenStream {
        let cursor_args = self.cursor_arg_tokens();
        quote! {
            (first + 1) as i64,
            #cursor_args
        }
    }

    pub fn destructure_tokens(&self) -> TokenStream {
        let column_name = self.column.name();

        let mut after_args = quote! {
            (id, #column_name)
        };
        let mut after_destruction = quote! {
            (Some(after.id), Some(after.#column_name))
        };
        let mut after_default = quote! {
            (None, None)
        };

        if self.column.is_id() {
            after_args = quote! {
                id
            };
            after_destruction = quote! {
                Some(after.id)
            };
            after_default = quote! {
                None
            };
        } else if self.column.is_optional() {
            after_destruction = quote! {
                (Some(after.id), after.#column_name)
            };
        }

        quote! {
            let es_entity::PaginatedQueryArgs { first, after } = cursor;
            let #after_args = if let Some(after) = after {
                #after_destruction
            } else {
                #after_default
            };
        }
    }

    #[cfg(feature = "graphql")]
    pub fn gql_cursor(&self) -> TokenStream {
        let ident = self.ident();
        quote! {
            impl es_entity::graphql::async_graphql::connection::CursorType for #ident {
                type Error = String;

                fn encode_cursor(&self) -> String {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let json = es_entity::prelude::serde_json::to_string(&self).expect("could not serialize token");
                    general_purpose::STANDARD_NO_PAD.encode(json.as_bytes())
                }

                fn decode_cursor(s: &str) -> Result<Self, Self::Error> {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let bytes = general_purpose::STANDARD_NO_PAD
                        .decode(s.as_bytes())
                        .map_err(|e| e.to_string())?;
                    let json = String::from_utf8(bytes).map_err(|e| e.to_string())?;
                    es_entity::prelude::serde_json::from_str(&json).map_err(|e| e.to_string())
                }
            }
        }
    }
}

impl ToTokens for CursorStruct<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let accessor = &self.column.accessor();
        let ident = self.ident();
        let id = &self.id;

        let (field, from_impl) = if self.column.is_id() {
            (quote! {}, quote! {})
        } else {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            (
                quote! {
                    pub #column_name: #column_type,
                },
                quote! {
                    #column_name: entity.#accessor.clone(),
                },
            )
        };

        tokens.append_all(quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct #ident {
                pub id: #id,
                #field
            }

            impl From<&#entity> for #ident {
                fn from(entity: &#entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                        #from_impl
                    }
                }
            }
        });
    }
}

pub struct ListByFn<'a> {
    ignore_prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    column: &'a Column,
    table_name: &'a str,
    query_error: syn::Ident,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    any_nested: bool,
    is_root: bool,
    post_hydrate_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    scope: Option<ScopeInfo<'a>>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListByFn<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            ignore_prefix: opts.table_prefix(),
            column,
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            query_error: opts.query_error(),
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            any_nested: opts.any_nested(),
            is_root: opts.is_root(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            scope: ScopeInfo::from_opts(opts),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    pub fn cursor(&'a self) -> CursorStruct<'a> {
        CursorStruct {
            column: self.column,
            id: self.id,
            entity: self.entity,
            cursor_mod: &self.cursor_mod,
        }
    }

    /// Delegating methods for the generated `Scoped{Repo}` bound view.
    /// Empty for unscoped repos.
    pub fn scoped_delegates(&'a self) -> TokenStream {
        let mut tokens = TokenStream::new();
        if self.scope.is_none() {
            return tokens;
        }
        let entity = self.entity;
        let column_name = self.column.name();
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let query_error = &self.query_error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let fn_name = syn::Ident::new(
                &format!(
                    "list_by_{}{}",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_by_{}{}_in_op",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error> {
                    self.repo.#fn_name(self.scope, cursor, direction).await
                }

                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error>
                   where
                       OP: #query_fn_op_traits
                {
                    self.repo.#fn_in_op(op, self.scope, cursor, direction).await
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
        tokens
    }
}

impl ToTokens for ListByFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let column_name = self.column.name();
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let query_error = &self.query_error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        let destructure_tokens = self.cursor().destructure_tokens();
        let select_columns = cursor.select_columns(None);

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let fn_name = syn::Ident::new(
                &format!(
                    "list_by_{}{}",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_by_{}{}_in_op",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            let forgettable_tbl_arg = if let Some(tbl) = self.forgettable_table_name {
                quote! { forgettable_tbl = #tbl, }
            } else {
                quote! {}
            };
            let root_arg = if self.is_root {
                quote! { root = true, }
            } else {
                quote! {}
            };

            let make_es_query = |query: &str, args: &TokenStream| -> TokenStream {
                if let Some(prefix) = self.ignore_prefix {
                    quote! {
                        es_entity::es_query!(
                            tbl_prefix = #prefix,
                            #forgettable_tbl_arg
                            #root_arg
                            #query,
                            #args
                        )
                    }
                } else {
                    quote! {
                        es_entity::es_query!(
                            entity = #entity,
                            #forgettable_tbl_arg
                            #root_arg
                            #query,
                            #args
                        )
                    }
                }
            };

            let build_query_arms = |scope: Option<&ScopeCol>| -> TokenStream {
                let mut query_arms = TokenStream::new();
                let offset = if scope.is_some() { 1 } else { 0 };
                for ascending in [true, false] {
                    let mut leading: Vec<String> = Vec::new();
                    if let Some(scope) = scope {
                        leading.push(scope.predicate(1));
                    }
                    let mut trailing: Vec<String> = Vec::new();
                    if delete == DeleteOption::No
                        && let Some(not_deleted) = not_deleted_predicate(self.delete)
                    {
                        trailing.push(not_deleted);
                    }
                    let branches = cursor.cursor_branches(offset, ascending);
                    let query = assemble_union_select(
                        &select_columns,
                        self.table_name,
                        &leading,
                        &branches,
                        &trailing,
                        &cursor.order_by(ascending),
                        offset + 1,
                    );
                    let cursor_args = cursor.cursor_arg_tokens();
                    let scope_args = scope.map(|s| s.arg_tokens()).unwrap_or_default();
                    let args = quote! {
                        #scope_args
                        (first + 1) as i64,
                        #cursor_args
                    };
                    let es_query_call = make_es_query(&query, &args);
                    let direction_pattern = if ascending {
                        quote! { es_entity::ListDirection::Ascending }
                    } else {
                        quote! { es_entity::ListDirection::Descending }
                    };
                    query_arms.append_all(quote! {
                        #direction_pattern => {
                            #es_query_call.fetch_n(op, first).await?
                        },
                    });
                }
                query_arms
            };
            let query_arms = build_query_arms(None);

            let (scope_fn_arg, scope_fn_pass, scope_convert) = match &self.scope {
                Some(scope) => (scope.fn_arg(), scope.fn_pass(), scope.convert()),
                None => (quote! {}, quote! {}, quote! {}),
            };
            let match_expr = if let Some(scope) = &self.scope {
                scope.dispatch(
                    quote! {
                        match direction {
                            #query_arms
                        }
                    },
                    |col| {
                        let scoped_query_arms = build_query_arms(Some(col));
                        quote! {
                            match direction {
                                #scoped_query_arms
                            }
                        }
                    },
                )
            } else {
                quote! {
                    match direction {
                        #query_arms
                    }
                }
            };

            #[cfg(feature = "instrument")]
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = {
                let entity_name = entity.to_string();
                let repo_name = &self.repo_name_snake;
                let span_name = format!("{}.list_by_{}", repo_name, column_name);
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, first, has_cursor, direction = tracing::field::debug(&direction), count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
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
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = (quote! {}, quote! {}, quote! {}, quote! {}, quote! {});

            let post_hydrate_check = if self.post_hydrate_error.is_some() {
                quote! {
                    for __entity in &entities {
                        self.execute_post_hydrate_hook(__entity).map_err(#query_error::PostHydrateError)?;
                    }
                }
            } else {
                quote! {}
            };

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    #scope_fn_arg
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error> {
                    self.#fn_in_op(#query_fn_get_op, #scope_fn_pass cursor, direction).await
                }

                #instrument_attr
                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    #scope_fn_arg
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error>
                   where
                       OP: #query_fn_op_traits
                 {
                    let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error> = async {
                        #scope_convert
                        #extract_has_cursor
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

    #[test]
    fn cursor_struct_by_id() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let by_column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let cursor = CursorStruct {
            column: &by_column,
            id: &id_type,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct EntityByIdCursor {
                pub id: EntityId,
            }

            impl From<&Entity> for EntityByIdCursor {
                fn from(entity: &Entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn cursor_struct_by_created_at() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let by_column = Column::for_created_at();
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let cursor = CursorStruct {
            column: &by_column,
            id: &id_type,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct EntityByCreatedAtCursor {
                pub id: EntityId,
                pub created_at: es_entity::prelude::chrono::DateTime<es_entity::prelude::chrono::Utc>,
            }

            impl From<&Entity> for EntityByCreatedAtCursor {
                fn from(entity: &Entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                        created_at: entity.events()
                            .entity_first_persisted_at()
                            .expect("entity not persisted")
                            .clone(),
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::SoftWithoutQueries,
            cursor_mod,
            any_nested: false,
            is_root: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_by_id(
                &self,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError> {
                self.list_by_id_in_op(self.pool(), cursor, direction).await
            }

            pub async fn list_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError> = async {
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let id = if let Some(after) = after {
                        Some(after.id)
                    } else {
                        None
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT id FROM entities WHERE (id > $2) AND ($2 IS NOT NULL) AND deleted = FALSE ORDER BY id ASC LIMIT $1) UNION ALL (SELECT id FROM entities WHERE ($2 IS NULL) AND deleted = FALSE ORDER BY id ASC LIMIT $1) ORDER BY id ASC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT id FROM entities WHERE (id < $2) AND ($2 IS NOT NULL) AND deleted = FALSE ORDER BY id DESC LIMIT $1) UNION ALL (SELECT id FROM entities WHERE ($2 IS NULL) AND deleted = FALSE ORDER BY id DESC LIMIT $1) ORDER BY id DESC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                    };

                    let end_cursor = entities.last().map(cursor_mod::EntityByIdCursor::from);
                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_with_soft_delete_include_deleted() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::Soft,
            cursor_mod,
            any_nested: false,
            is_root: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();
        assert!(token_str.contains("list_by_id_include_deleted"));
    }

    #[test]
    fn list_by_fn_name() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new(
            syn::Ident::new("name", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            is_root: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_by_name(
                &self,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByNameCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByNameCursor>, EntityQueryError> {
                self.list_by_name_in_op(self.pool(), cursor, direction).await
            }

            pub async fn list_by_name_in_op<'a, OP>(
                &self,
                op: OP,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByNameCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByNameCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByNameCursor>, EntityQueryError> = async {
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let (id, name) = if let Some(after) = after {
                        (Some(after.id), Some(after.name))
                    } else {
                        (None, None)
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) AND ($2 IS NOT NULL) ORDER BY name ASC, id ASC LIMIT $1) UNION ALL (SELECT name, id FROM entities WHERE ($2 IS NULL) ORDER BY name ASC, id ASC LIMIT $1) ORDER BY name ASC, id ASC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                name as Option<String>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT name, id FROM entities WHERE ((name, id) < ($3, $2)) AND ($2 IS NOT NULL) ORDER BY name DESC, id DESC LIMIT $1) UNION ALL (SELECT name, id FROM entities WHERE ($2 IS NULL) ORDER BY name DESC, id DESC LIMIT $1) ORDER BY name DESC, id DESC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                name as Option<String>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                    };

                    let end_cursor = entities.last().map(cursor_mod::EntityByNameCursor::from);
                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_optional_column() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new(
            syn::Ident::new("value", proc_macro2::Span::call_site()),
            syn::parse_str("Option<rust_decimal::Decimal>").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            is_root: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_by_value(
                &self,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByValueCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError> {
                self.list_by_value_in_op(self.pool(), cursor, direction).await
            }

            pub async fn list_by_value_in_op<'a, OP>(
                &self,
                op: OP,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByValueCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError> = async {
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let (id, value) = if let Some(after) = after {
                        (Some(after.id), after.value)
                    } else {
                        (None, None)
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT value, id FROM entities WHERE ((value, id) > ($3, $2)) AND ($2 IS NOT NULL AND $3 IS NOT NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ((value IS NOT NULL OR id > $2)) AND ($2 IS NOT NULL AND $3 IS NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ($2 IS NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                value as Option<rust_decimal::Decimal>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT value, id FROM entities WHERE ((value IS NULL OR (value, id) < ($3, $2))) AND ($2 IS NOT NULL AND $3 IS NOT NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ((value IS NULL AND id < $2)) AND ($2 IS NOT NULL AND $3 IS NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ($2 IS NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                value as Option<rust_decimal::Decimal>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                    };

                    let end_cursor = entities.last().map(cursor_mod::EntityByValueCursor::from);
                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_nullable_attribute_emits_nullable_aware_sql_for_non_option_type() {
        // The `nullable` attribute lets non-Option<T> Rust types (e.g. domain
        // enums whose custom sqlx::Encode writes NULL for one variant) opt
        // into the nullable-aware cursor SQL form. The Rust type is NOT
        // syntactically Option<T>, but the emitted SQL matches what an
        // `Option<T>` column gets: the unified 3-branch `UNION ALL` query with
        // `NULLS FIRST/LAST` ordering and the direction-aware NULL branches.
        //
        // Crucially, the AfterNull branch's gate (`$3 IS NULL`) dispatches on
        // the *encoded SQL* NULL-ness of the cursor value in the database —
        // where it is always visible — even though Rust cannot see it from the
        // non-`Option` type. This is what lets `nullable`-annotated columns use
        // the sargable branch form instead of the non-sargable `COALESCE`
        // catch-all (the eliminated `CursorState::AfterMaybeNull`), making them
        // index-friendly for the first time.
        //
        // The query parameter cast (else branch of cursor_arg_tokens) still
        // wraps the Rust type in Option<...> because is_optional() remains
        // false — the type itself drives binding, while is_nullable_column()
        // drives SQL shape.
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new_nullable(
            syn::Ident::new("value", proc_macro2::Span::call_site()),
            syn::parse_str("DomainEnum").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            is_root: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_by_value(
                &self,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByValueCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError> {
                self.list_by_value_in_op(self.pool(), cursor, direction).await
            }

            pub async fn list_by_value_in_op<'a, OP>(
                &self,
                op: OP,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByValueCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByValueCursor>, EntityQueryError> = async {
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let (id, value) = if let Some(after) = after {
                        (Some(after.id), Some(after.value))
                    } else {
                        (None, None)
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT value, id FROM entities WHERE ((value, id) > ($3, $2)) AND ($2 IS NOT NULL AND $3 IS NOT NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ((value IS NOT NULL OR id > $2)) AND ($2 IS NOT NULL AND $3 IS NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ($2 IS NULL) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                value as Option<DomainEnum>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT value, id FROM entities WHERE ((value IS NULL OR (value, id) < ($3, $2))) AND ($2 IS NOT NULL AND $3 IS NOT NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ((value IS NULL AND id < $2)) AND ($2 IS NOT NULL AND $3 IS NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) UNION ALL (SELECT value, id FROM entities WHERE ($2 IS NULL) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1",
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                value as Option<DomainEnum>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                    };

                    let end_cursor = entities.last().map(cursor_mod::EntityByValueCursor::from);
                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
