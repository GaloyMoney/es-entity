use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{combo_cursor::ComboCursor, list_for_fn::ListForFn, options::*};

pub struct Filter<'a> {
    columns: Vec<&'a Column>,
    entity: &'a syn::Ident,
}

impl<'a> Filter<'a> {
    pub fn new(opts: &'a RepositoryOptions, columns: Vec<&'a Column>) -> Self {
        Self {
            entity: opts.entity(),
            columns,
        }
    }

    pub fn ident(&self) -> syn::Ident {
        let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
        syn::Ident::new(
            &format!("{entity_name}_filter").to_case(Case::UpperCamel),
            Span::call_site(),
        )
    }

    fn tag(column: &Column) -> syn::Ident {
        let tag_name = format!("with_{}", column.name()).to_case(Case::UpperCamel);
        syn::Ident::new(&tag_name, Span::call_site())
    }

    pub fn variants(&self) -> TokenStream {
        let variants = self
            .columns
            .iter()
            .map(|column| {
                let tag = Self::tag(column);
                let ty = column.ty();
                quote! {
                    #tag(#ty),
                }
            })
            .collect::<TokenStream>();

        quote! {
            #variants
        }
    }
}

impl ToTokens for Filter<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = self.ident();
        let variants = self.variants();

        tokens.append_all(quote! {
            #[derive(Debug)]
            #[allow(clippy::enum_variant_names)]
            pub enum #ident {
                NoFilter,
                #variants
            }
        });
    }
}

pub struct ListForFilterFn<'a> {
    pub filter: Filter<'a>,
    entity: &'a syn::Ident,
    error: &'a syn::Type,
    list_for_fns: &'a Vec<ListForFn<'a>>,
    by_columns: Vec<&'a Column>,
    cursor: &'a ComboCursor<'a>,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListForFilterFn<'a> {
    pub fn new(
        opts: &'a RepositoryOptions,
        list_for_fns: &'a Vec<ListForFn<'a>>,
        for_columns: Vec<&'a Column>,
        by_columns: Vec<&'a Column>,
        cursor: &'a ComboCursor<'a>,
    ) -> Self {
        Self {
            filter: Filter::new(opts, for_columns.clone()),
            entity: opts.entity(),
            error: opts.err(),
            list_for_fns,
            by_columns,
            cursor,
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for ListForFilterFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let filter_name = self.filter.ident();
        let sort_by_name = self.cursor.sort_by_name();
        let cursor_ident = self.cursor.ident();

        let entity = self.entity;
        let error = self.error;

        let cursor_mod = &self.cursor_mod;

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let variants = self.list_for_fns.iter().map(|f| {
                let fn_name = syn::Ident::new(
                    &format!(
                        "list_for_{}_by_{}{}",
                        f.for_column.name(),
                        f.by_column.name(),
                        delete.include_deletion_fn_postfix()
                    ),
                    Span::call_site(),
                );
                let filter_variant = Filter::tag(f.for_column);
                let by_variant = syn::Ident::new(
                    &format!("{}", f.by_column.name()).to_case(Case::UpperCamel),
                    Span::call_site(),
                );
                let inner_cursor_ident = f.cursor().ident();
                quote! {
                    (#filter_name::#filter_variant(filter_value), #sort_by_name::#by_variant) => {
                        let after = after.map(#cursor_mod::#inner_cursor_ident::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.#fn_name(filter_value, query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(#cursor_mod::#cursor_ident::from)
                        }
                    }
                }
            }).chain(
            self.by_columns.iter().map(|b| {
                let by_variant = syn::Ident::new(
                    &format!("{}", b.name()).to_case(Case::UpperCamel),
                    Span::call_site(),
                );
                let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
                let inner_cursor_ident = syn::Ident::new(
                    &format!("{}_by_{}_cursor", entity_name, b.name()).to_case(Case::UpperCamel)
                    , Span::call_site());
                let no_filter_fn_name = syn::Ident::new(
                    &format!(
                        "list_by_{}{}",
                        b.name(),
                        delete.include_deletion_fn_postfix()
                    ),
                    Span::call_site(),
                );
                quote! {
                    (#filter_name::NoFilter, #sort_by_name::#by_variant) => {
                        let after = after.map(#cursor_mod::#inner_cursor_ident::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.#no_filter_fn_name(query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(#cursor_mod::#cursor_ident::from)
                        }
                    }
                }
            }));
            let fn_name = syn::Ident::new(
                &format!("list_for_filter{}", delete.include_deletion_fn_postfix()),
                Span::call_site(),
            );

            #[cfg(feature = "instrument")]
            let (instrument_attr, extract_has_cursor, record_fields, record_results, error_recording) = {
                let entity_name = self.entity.to_string();
                let repo_name = &self.repo_name_snake;
                let span_name = format!("{}.list_for_filter", repo_name);
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, filter = tracing::field::debug(&filter), sort_by = tracing::field::debug(&sort.by), direction = tracing::field::debug(&sort.direction), first, has_cursor, count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
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
                            tracing::Span::current().record("exception.message", tracing::field::display(e));
                            tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                        }
                    },
                )
            };
            #[cfg(not(feature = "instrument"))]
            let (instrument_attr, extract_has_cursor, record_fields, record_results, error_recording) =
                (quote! {}, quote! {}, quote! {}, quote! {}, quote! {});

            tokens.append_all(quote! {
                #instrument_attr
                pub async fn #fn_name(
                    &self,
                    filter: #filter_name,
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
                        let res = match (filter, by) {
                            #(#variants)*
                        };

                        #record_results

                        Ok(res)
                    }.await;
                    
                    #error_recording
                    __result
                }
            });

            if delete == self.delete {
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
    fn filter_enum() {
        let entity = Ident::new("Order", Span::call_site());
        let customer_id_column = Column::new(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
        );
        let status_column = Column::new(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
        );

        // Create a minimal Filter manually
        let filter = Filter {
            entity: &entity,
            columns: vec![&customer_id_column, &status_column],
        };

        let mut tokens = TokenStream::new();
        filter.to_tokens(&mut tokens);

        // @ claude this unit test is failing - why?
        let expected = quote! {
            #[derive(Debug)]
            #[allow(clippy::enum_variant_names)]
            pub enum OrdersFilter {
                NoFilter,
                WithCustomerId(CustomerId),
                WithStatus(OrderStatus),
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_for_filter_function_generation() {
        use crate::repo::{combo_cursor::ComboCursor, list_by_fn::CursorStruct};

        let entity = Ident::new("Order", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let id = syn::Ident::new("OrderId", proc_macro2::Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        // Create columns
        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let customer_id_column = Column::new(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
        );
        let status_column = Column::new(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
        );

        let filter = Filter {
            entity: &entity,
            columns: vec![&customer_id_column, &status_column],
        };

        let list_for_customer_id_by_id = ListForFn::new_test(
            &customer_id_column,
            &id_column,
            &entity,
            &id,
            "orders",
            &error,
            cursor_mod.clone(),
        );

        let list_for_status_by_id = ListForFn::new_test(
            &status_column,
            &id_column,
            &entity,
            &id,
            "orders",
            &error,
            cursor_mod.clone(),
        );

        let list_for_fns = vec![list_for_customer_id_by_id, list_for_status_by_id];
        let by_columns = vec![&id_column, &customer_id_column, &status_column];

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let customer_cursor = CursorStruct {
            column: &customer_id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let status_cursor = CursorStruct {
            column: &status_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let combo_cursor =
            ComboCursor::new_test(&entity, vec![id_cursor, customer_cursor, status_cursor]);

        let list_for_filter_fn = ListForFilterFn {
            filter,
            entity: &entity,
            error: &error,
            list_for_fns: &list_for_fns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        list_for_filter_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_filter(
                &self,
                filter: OrdersFilter,
                sort: es_entity::Sort<OrdersSortBy>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrdersCursor>,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersCursor>, es_entity::EsRepoError>
                where es_entity::EsRepoError: From<es_entity::CursorDestructureError>
            {
                let es_entity::Sort { by, direction } = sort;
                let es_entity::PaginatedQueryArgs { first, after } = cursor;

                use cursor_mod::OrdersCursor;
                let res = match (filter, by) {
                    (OrdersFilter::WithCustomerId(filter_value), OrdersSortBy::Id) => {
                        let after = after.map(cursor_mod::OrdersByIdCursor::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.list_for_customer_id_by_id(filter_value, query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
                        }
                    }
                    (OrdersFilter::WithStatus(filter_value), OrdersSortBy::Id) => {
                        let after = after.map(cursor_mod::OrdersByIdCursor::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.list_for_status_by_id(filter_value, query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
                        }
                    }
                    (OrdersFilter::NoFilter, OrdersSortBy::Id) => {
                        let after = after.map(cursor_mod::OrdersByIdCursor::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.list_by_id(query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
                        }
                    }
                    (OrdersFilter::NoFilter, OrdersSortBy::CustomerId) => {
                        let after = after.map(cursor_mod::OrdersByCustomerIdCursor::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.list_by_customer_id(query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
                        }
                    }
                    (OrdersFilter::NoFilter, OrdersSortBy::Status) => {
                        let after = after.map(cursor_mod::OrdersByStatusCursor::try_from).transpose()?;
                        let query = es_entity::PaginatedQueryArgs { first, after };

                        let es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        } = self.list_by_status(query, direction).await?;
                        es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor: end_cursor.map(cursor_mod::OrdersCursor::from)
                        }
                    }
                };

                Ok(res)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
