use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{
    list_by_fn::{CursorStruct, assemble_union_select, not_deleted_predicate},
    options::*,
    scope::ScopeInfo,
};

pub struct ListForFn<'a> {
    ignore_prefix: Option<&'a syn::LitStr>,
    pub for_column: &'a Column,
    pub by_column: &'a Column,
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    query_error: syn::Ident,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    any_nested: bool,
    post_hydrate_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    scope: Option<ScopeInfo<'a>>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListForFn<'a> {
    pub fn new(for_column: &'a Column, by_column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            ignore_prefix: opts.table_prefix(),
            for_column,
            by_column,
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            query_error: opts.query_error(),
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            any_nested: opts.any_nested(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            scope: ScopeInfo::from_opts(opts),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    pub fn cursor(&'a self) -> CursorStruct<'a> {
        CursorStruct {
            column: self.by_column,
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
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let error = &self.query_error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);

        let by_column_name = self.by_column.name();
        let for_column_name = self.for_column.name();
        let filter_arg_name = syn::Ident::new(
            &format!("filter_{}", self.for_column.name()),
            Span::call_site(),
        );
        let (_for_column_type, for_impl_expr, _for_access_expr) = self.for_column.ty_for_find_by();

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let fn_name = syn::Ident::new(
                &format!(
                    "list_for_{}_by_{}{}",
                    for_column_name,
                    by_column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_for_{}_by_{}{}_in_op",
                    for_column_name,
                    by_column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    #filter_arg_name: #for_impl_expr,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                    self.repo.#fn_name(self.scope, #filter_arg_name, cursor, direction).await
                }

                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    #filter_arg_name: #for_impl_expr,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    where
                        OP: #query_fn_op_traits
                {
                    self.repo.#fn_in_op(op, self.scope, #filter_arg_name, cursor, direction).await
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
        tokens
    }
}

impl ToTokens for ListForFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let error = &self.query_error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        let by_column_name = self.by_column.name();

        let for_column_name = self.for_column.name();
        let filter_arg_name = syn::Ident::new(
            &format!("filter_{}", self.for_column.name()),
            Span::call_site(),
        );
        let (for_column_type, for_impl_expr, for_access_expr) = self.for_column.ty_for_find_by();

        let destructure_tokens = self.cursor().destructure_tokens();
        let select_columns = cursor.select_columns(Some(for_column_name));

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let fn_name = syn::Ident::new(
                &format!(
                    "list_for_{}_by_{}{}",
                    for_column_name,
                    by_column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_for_{}_by_{}{}_in_op",
                    for_column_name,
                    by_column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            let filter_op = if self.for_column.is_optional() {
                "IS NOT DISTINCT FROM"
            } else {
                "="
            };

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

            let build_query_arms = |scope: Option<&ScopeInfo>| -> TokenStream {
                let mut query_arms = TokenStream::new();
                let offset = if scope.is_some() { 2 } else { 1 };
                for ascending in [true, false] {
                    let mut leading: Vec<String> =
                        vec![format!("({for_column_name} {filter_op} $1)")];
                    if let Some(scope) = scope {
                        leading.push(scope.predicate(2));
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
                        #filter_arg_name as &#for_column_type,
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
                let scoped_query_arms = build_query_arms(Some(scope));
                scope.dispatch(
                    quote! {
                        match direction {
                            #query_arms
                        }
                    },
                    quote! {
                        match direction {
                            #scoped_query_arms
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
                let span_name = format!(
                    "{}.list_for_{}_by_{}",
                    repo_name, for_column_name, by_column_name
                );
                let filter_field_name = format!("query_{}", filter_arg_name);
                let filter_field_ident =
                    syn::Ident::new(&filter_field_name, proc_macro2::Span::call_site());
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #filter_field_ident = tracing::field::Empty, first, has_cursor, direction = tracing::field::debug(&direction), count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                    },
                    quote! {
                        let has_cursor = cursor.after.is_some();
                    },
                    quote! {
                        tracing::Span::current().record(#filter_field_name, tracing::field::debug(&#filter_arg_name));
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
                        self.execute_post_hydrate_hook(__entity).map_err(#error::PostHydrateError)?;
                    }
                }
            } else {
                quote! {}
            };

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    #scope_fn_arg
                    #filter_arg_name: #for_impl_expr,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                    self.#fn_in_op(#query_fn_get_op, #scope_fn_pass #filter_arg_name, cursor, direction).await
                }

                #instrument_attr
                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    #scope_fn_arg
                    #filter_arg_name: #for_impl_expr,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    where
                        OP: #query_fn_op_traits
                {
                    let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> = async {
                        #scope_convert
                        #extract_has_cursor
                        let #filter_arg_name = #filter_arg_name.#for_access_expr;
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
    fn list_for_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let id = syn::Ident::new("EntityId", proc_macro2::Span::call_site());
        let by_column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let for_column = Column::new(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("Uuid").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListForFn {
            ignore_prefix: None,
            entity: &entity,
            id: &id,
            for_column: &for_column,
            by_column: &by_column,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_customer_id_by_id(
                &self,
                filter_customer_id: impl std::borrow::Borrow<Uuid>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError> {
                self.list_for_customer_id_by_id_in_op(self.pool(), filter_customer_id, cursor, direction).await
            }

            pub async fn list_for_customer_id_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                filter_customer_id: impl std::borrow::Borrow<Uuid>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByIdCursor>, EntityQueryError> = async {
                    let filter_customer_id = filter_customer_id.borrow();
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
                                "(SELECT customer_id, id FROM entities WHERE (customer_id = $1) AND (id > $3) AND ($3 IS NOT NULL) ORDER BY id ASC LIMIT $2) UNION ALL (SELECT customer_id, id FROM entities WHERE (customer_id = $1) AND ($3 IS NULL) ORDER BY id ASC LIMIT $2) ORDER BY id ASC LIMIT $2",
                                filter_customer_id as &Uuid,
                                (first + 1) as i64,
                                id as Option<EntityId>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT customer_id, id FROM entities WHERE (customer_id = $1) AND (id < $3) AND ($3 IS NOT NULL) ORDER BY id DESC LIMIT $2) UNION ALL (SELECT customer_id, id FROM entities WHERE (customer_id = $1) AND ($3 IS NULL) ORDER BY id DESC LIMIT $2) ORDER BY id DESC LIMIT $2",
                                filter_customer_id as &Uuid,
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
    fn list_same_column() {
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let id = syn::Ident::new("EntityId", proc_macro2::Span::call_site());
        let column = Column::new(
            syn::Ident::new("email", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListForFn {
            ignore_prefix: None,
            entity: &entity,
            id: &id,
            for_column: &column,
            by_column: &column,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            scope: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_email_by_email(
                &self,
                filter_email: impl std::convert::AsRef<str>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByEmailCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByEmailCursor>, EntityQueryError> {
                self.list_for_email_by_email_in_op(self.pool(), filter_email, cursor, direction).await
            }

            pub async fn list_for_email_by_email_in_op<'a, OP>(
                &self,
                op: OP,
                filter_email: impl std::convert::AsRef<str>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntityByEmailCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByEmailCursor>, EntityQueryError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntityByEmailCursor>, EntityQueryError> = async {
                    let filter_email = filter_email.as_ref();
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let (id, email) = if let Some(after) = after {
                        (Some(after.id), Some(after.email))
                    } else {
                        (None, None)
                    };
                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT email, id FROM entities WHERE (email = $1) AND ((email, id) > ($4, $3)) AND ($3 IS NOT NULL) ORDER BY email ASC, id ASC LIMIT $2) UNION ALL (SELECT email, id FROM entities WHERE (email = $1) AND ($3 IS NULL) ORDER BY email ASC, id ASC LIMIT $2) ORDER BY email ASC, id ASC LIMIT $2",
                                filter_email as &str,
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                email as Option<String>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Entity,
                                "(SELECT email, id FROM entities WHERE (email = $1) AND ((email, id) < ($4, $3)) AND ($3 IS NOT NULL) ORDER BY email DESC, id DESC LIMIT $2) UNION ALL (SELECT email, id FROM entities WHERE (email = $1) AND ($3 IS NULL) ORDER BY email DESC, id DESC LIMIT $2) ORDER BY email DESC, id DESC LIMIT $2",
                                filter_email as &str,
                                (first + 1) as i64,
                                id as Option<EntityId>,
                                email as Option<String>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                    };

                    let end_cursor = entities.last().map(cursor_mod::EntityByEmailCursor::from);
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
