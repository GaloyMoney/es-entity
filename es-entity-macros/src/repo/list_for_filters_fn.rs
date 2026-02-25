use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{combo_cursor::ComboCursor, list_by_fn::CursorStruct, options::*};

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
        let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
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

    fn where_clause_fragment(column: &Column, idx: u32) -> String {
        let col_name = column.name();
        let param = format!("${idx}");
        format!("COALESCE({col_name} = {param}, {param} IS NULL)")
    }

    fn filter_arg_tokens(column: &Column) -> TokenStream {
        let name = syn::Ident::new(&format!("filter_{}", column.name()), Span::call_site());
        let ty = column.ty();
        if let syn::Type::Path(type_path) = ty
            && type_path.path.is_ident("String")
        {
            quote! {
                #name as Option<String>,
            }
        } else {
            quote! {
                #name as Option<#ty>,
            }
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
    error: &'a syn::Type,
    for_columns: Vec<&'a Column>,
    by_columns: Vec<&'a Column>,
    cursor: &'a ComboCursor<'a>,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    table_name: &'a str,
    ignore_prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    any_nested: bool,
    include_deleted_queries: bool,
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
            error: opts.err(),
            for_columns,
            by_columns,
            cursor,
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            table_name: opts.table_name(),
            ignore_prefix: opts.table_prefix(),
            id: opts.id(),
            any_nested: opts.any_nested(),
            include_deleted_queries: opts.include_deleted_queries,
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    fn generate_proxy_body(&self, by_col: &Column, delete: DeleteOption) -> TokenStream {
        let by_col_name = by_col.name();
        let delete_postfix = delete.include_deletion_fn_postfix();

        let list_by_fn = syn::Ident::new(
            &format!("list_by_{}{}", by_col_name, delete_postfix),
            Span::call_site(),
        );

        if self.for_columns.is_empty() {
            return quote! { self.#list_by_fn(query, direction).await? };
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
                            self.#fn_name(filters.#for_col_name.unwrap(), query, direction).await?
                        }
                    }
                } else {
                    quote! {
                        else if #(#others_none)&&* {
                            self.#fn_name(filters.#for_col_name.unwrap(), query, direction).await?
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
                    self.#list_for_filters_fn(filters, query, direction).await?
                }
            }
        } else {
            quote! {}
        };

        quote! {
            if #(#all_none_checks)&&* {
                self.#list_by_fn(query, direction).await?
            }
            #single_filter_branches
            #multi_filter_fallback
        }
    }

    fn generate_by_fn(&self, by_column: &'a Column, delete: DeleteOption) -> TokenStream {
        let entity = self.entity;
        let error = self.error;
        let cursor_mod = &self.cursor_mod;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        let by_column_name = by_column.name();
        let cursor_struct = CursorStruct {
            column: by_column,
            id: self.id,
            entity: self.entity,
            cursor_mod: &self.cursor_mod,
        };
        let cursor_ident = cursor_struct.ident();

        let n_filters = self.for_columns.len() as u32;

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
        let filter_field_names: Vec<_> = self
            .for_columns
            .iter()
            .map(|c| {
                let col_name = c.name();
                let filter_name =
                    syn::Ident::new(&format!("filter_{}", col_name), Span::call_site());
                (col_name.clone(), filter_name)
            })
            .collect();

        let destructure_filters: TokenStream = filter_field_names
            .iter()
            .map(|(col_name, filter_name)| {
                quote! {
                    let #filter_name = filters.#col_name;
                }
            })
            .collect();

        // Generate WHERE clause fragments
        let where_fragments: Vec<String> = self
            .for_columns
            .iter()
            .enumerate()
            .map(|(i, col)| FiltersStruct::where_clause_fragment(col, (i + 1) as u32))
            .collect();

        let filter_where = if where_fragments.is_empty() {
            String::new()
        } else {
            format!("{} AND ", where_fragments.join(" AND "))
        };

        // Generate filter arg bindings for es_query!
        let filter_arg_bindings: TokenStream = self
            .for_columns
            .iter()
            .map(|col| FiltersStruct::filter_arg_tokens(col))
            .collect();

        let asc_query = format!(
            r#"SELECT {} FROM {} WHERE {}({}){} ORDER BY {} LIMIT ${}"#,
            select_columns,
            self.table_name,
            filter_where,
            cursor_struct.condition(n_filters, true),
            if delete == DeleteOption::No {
                self.delete.not_deleted_condition()
            } else {
                ""
            },
            cursor_struct.order_by(true),
            n_filters + 1,
        );
        let desc_query = format!(
            r#"SELECT {} FROM {} WHERE {}({}){} ORDER BY {} LIMIT ${}"#,
            select_columns,
            self.table_name,
            filter_where,
            cursor_struct.condition(n_filters, false),
            if delete == DeleteOption::No {
                self.delete.not_deleted_condition()
            } else {
                ""
            },
            cursor_struct.order_by(false),
            n_filters + 1,
        );

        let es_query_asc_call = if let Some(prefix) = self.ignore_prefix {
            quote! {
                es_entity::es_query!(
                    tbl_prefix = #prefix,
                    #asc_query,
                    #filter_arg_bindings
                    #cursor_arg_tokens
                )
            }
        } else {
            quote! {
                es_entity::es_query!(
                    entity = #entity,
                    #asc_query,
                    #filter_arg_bindings
                    #cursor_arg_tokens
                )
            }
        };

        let es_query_desc_call = if let Some(prefix) = self.ignore_prefix {
            quote! {
                es_entity::es_query!(
                    tbl_prefix = #prefix,
                    #desc_query,
                    #filter_arg_bindings
                    #cursor_arg_tokens
                )
            }
        } else {
            quote! {
                es_entity::es_query!(
                    entity = #entity,
                    #desc_query,
                    #filter_arg_bindings
                    #cursor_arg_tokens
                )
            }
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

        quote! {
            pub async fn #fn_name(
                &self,
                filters: #filters_ident,
                cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                self.#fn_in_op(#query_fn_get_op, filters, cursor, direction).await
            }

            #instrument_attr
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
                let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> = async {
                    #extract_has_cursor
                    #destructure_filters
                    #destructure_tokens
                    #record_fields

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            #es_query_asc_call.fetch_n(op, first).await?
                        },
                        es_entity::ListDirection::Descending => {
                            #es_query_desc_call.fetch_n(op, first).await?
                        }
                    };

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
        let error = self.error;
        let cursor_mod = &self.cursor_mod;

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
                        let entity_name =
                            pluralizer::pluralize(&format!("{}", self.entity), 2, false);
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
                    filters: #filters_name,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    where #error: From<es_entity::CursorDestructureError>
                {
                    let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> = async {
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

            if delete == self.delete
                || (self.delete == DeleteOption::Soft && !self.include_deleted_queries)
            {
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
            pub struct OrdersFilters {
                pub customer_id: Option<CustomerId>,
                pub status: Option<OrderStatus>,
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_for_filters_function_generation() {
        let entity = Ident::new("Order", Span::call_site());
        let error: syn::Type = syn::parse_str("es_entity::EsRepoError").unwrap();
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
            error: &error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            any_nested: false,
            include_deleted_queries: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filters_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_filters_by_id(
                &self,
                filters: OrdersFilters,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrdersByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersByIdCursor>, es_entity::EsRepoError> {
                self.list_for_filters_by_id_in_op(self.pool(), filters, cursor, direction).await
            }

            pub async fn list_for_filters_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                filters: OrdersFilters,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrdersByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersByIdCursor>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersByIdCursor>, es_entity::EsRepoError> = async {
                    let filter_customer_id = filters.customer_id;
                    let filter_status = filters.status;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;
                    let id = if let Some(after) = after {
                        Some(after.id)
                    } else {
                        None
                    };

                    let (entities, has_next_page) = match direction {
                        es_entity::ListDirection::Ascending => {
                            es_entity::es_query!(
                                entity = Order,
                                "SELECT id FROM orders WHERE COALESCE(customer_id = $1, $1 IS NULL) AND COALESCE(status = $2, $2 IS NULL) AND (COALESCE(id > $4, true)) ORDER BY id ASC LIMIT $3",
                                filter_customer_id as Option<CustomerId>,
                                filter_status as Option<OrderStatus>,
                                (first + 1) as i64,
                                id as Option<OrderId>,
                            )
                                .fetch_n(op, first)
                                .await?
                        },
                        es_entity::ListDirection::Descending => {
                            es_entity::es_query!(
                                entity = Order,
                                "SELECT id FROM orders WHERE COALESCE(customer_id = $1, $1 IS NULL) AND COALESCE(status = $2, $2 IS NULL) AND (COALESCE(id < $4, true)) ORDER BY id DESC LIMIT $3",
                                filter_customer_id as Option<CustomerId>,
                                filter_status as Option<OrderStatus>,
                                (first + 1) as i64,
                                id as Option<OrderId>,
                            )
                                .fetch_n(op, first)
                                .await?
                        }
                    };

                    let end_cursor = entities.last().map(cursor_mod::OrdersByIdCursor::from);

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
                filters: OrdersFilters,
                sort: es_entity::Sort<OrdersSortBy>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrdersCursor>,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersCursor>, es_entity::EsRepoError>
                where es_entity::EsRepoError: From<es_entity::CursorDestructureError>
            {
                let __result: Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersCursor>, es_entity::EsRepoError> = async {
                    let es_entity::Sort { by, direction } = sort;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;

                    use cursor_mod::OrdersCursor;
                    let res = match by {
                        OrdersSortBy::Id => {
                            let after = after.map(cursor_mod::OrdersByIdCursor::try_from).transpose()?;
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
                                end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
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
        let error: syn::Type = syn::parse_str("es_entity::EsRepoError").unwrap();
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
            error: &error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            any_nested: false,
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
        let error: syn::Type = syn::parse_str("es_entity::EsRepoError").unwrap();
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
            error: &error,
            for_columns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            table_name: "orders",
            ignore_prefix: None,
            id: &id,
            any_nested: false,
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
}
