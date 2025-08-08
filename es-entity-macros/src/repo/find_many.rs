use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{combo_cursor::ComboCursor, list_for_fn::ListForFn, options::*};

pub struct ManyFilter<'a> {
    columns: Vec<&'a Column>,
    entity: &'a syn::Ident,
}

impl<'a> ManyFilter<'a> {
    pub fn new(opts: &'a RepositoryOptions, columns: Vec<&'a Column>) -> Self {
        Self {
            entity: opts.entity(),
            columns,
        }
    }

    pub fn ident(&self) -> syn::Ident {
        let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
        syn::Ident::new(
            &format!("find_many_{entity_name}").to_case(Case::UpperCamel),
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

impl ToTokens for ManyFilter<'_> {
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

pub struct FindManyFn<'a> {
    pub filter: ManyFilter<'a>,
    entity: &'a syn::Ident,
    error: &'a syn::Type,
    list_for_fns: &'a Vec<ListForFn<'a>>,
    by_columns: Vec<&'a Column>,
    cursor: &'a ComboCursor<'a>,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
}

impl<'a> FindManyFn<'a> {
    pub fn new(
        opts: &'a RepositoryOptions,
        list_for_fns: &'a Vec<ListForFn<'a>>,
        for_columns: Vec<&'a Column>,
        by_columns: Vec<&'a Column>,
        cursor: &'a ComboCursor<'a>,
    ) -> Self {
        Self {
            filter: ManyFilter::new(opts, for_columns.clone()),
            entity: opts.entity(),
            error: opts.err(),
            list_for_fns,
            by_columns,
            cursor,
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
        }
    }
}

impl ToTokens for FindManyFn<'_> {
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
                let filter_variant = ManyFilter::tag(f.for_column);
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
                &format!("find_many{}", delete.include_deletion_fn_postfix()),
                Span::call_site(),
            );
            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    filter: #filter_name,
                    sort: es_entity::Sort<#sort_by_name>,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error>
                    where #error: From<es_entity::CursorDestructureError>
                {
                    let es_entity::Sort { by, direction } = sort;
                    let es_entity::PaginatedQueryArgs { first, after } = cursor;

                    use #cursor_mod::#cursor_ident;
                    let res = match (filter, by) {
                        #(#variants)*
                    };

                    Ok(res)
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
    fn many_filter_enum() {
        let entity = Ident::new("Order", Span::call_site());
        let customer_id_column = Column::new(
            syn::Ident::new("customer_id", proc_macro2::Span::call_site()),
            syn::parse_str("CustomerId").unwrap(),
        );
        let status_column = Column::new(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("OrderStatus").unwrap(),
        );

        // Create a minimal ManyFilter manually
        let filter = ManyFilter {
            entity: &entity,
            columns: vec![&customer_id_column, &status_column],
        };

        let mut tokens = TokenStream::new();
        filter.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug)]
            #[allow(clippy::enum_variant_names)]
            pub enum FindManyOrders {
                NoFilter,
                WithCustomerId(CustomerId),
                WithStatus(OrderStatus),
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn find_many_function_generation() {
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

        // Create a ManyFilter
        let filter = ManyFilter {
            entity: &entity,
            columns: vec![&customer_id_column, &status_column],
        };

        // Create list_for functions using test constructor
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

        // Create cursor structs for combo cursor
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

        // Create combo cursor using test constructor
        let combo_cursor =
            ComboCursor::new_test(&entity, vec![id_cursor, customer_cursor, status_cursor]);

        let find_many_fn = FindManyFn {
            filter,
            entity: &entity,
            error: &error,
            list_for_fns: &list_for_fns,
            by_columns,
            cursor: &combo_cursor,
            delete: DeleteOption::No,
            cursor_mod: cursor_mod.clone(),
        };

        let mut tokens = TokenStream::new();
        find_many_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_many(
                &self,
                filter: FindManyOrders,
                sort: es_entity::Sort<OrdersSortBy>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::OrdersCursor>,
            ) -> Result<es_entity::PaginatedQueryRet<Order, cursor_mod::OrdersCursor>, es_entity::EsRepoError>
                where es_entity::EsRepoError: From<es_entity::CursorDestructureError>
            {
                let es_entity::Sort { by, direction } = sort;
                let es_entity::PaginatedQueryArgs { first, after } = cursor;

                use cursor_mod::OrdersCursor;
                let res = match (filter, by) {
                    (FindManyOrders::WithCustomerId(filter_value), OrdersSortBy::Id) => {
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
                    (FindManyOrders::WithStatus(filter_value), OrdersSortBy::Id) => {
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
                    (FindManyOrders::NoFilter, OrdersSortBy::Id) => {
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
                    (FindManyOrders::NoFilter, OrdersSortBy::CustomerId) => {
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
                    (FindManyOrders::NoFilter, OrdersSortBy::Status) => {
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
