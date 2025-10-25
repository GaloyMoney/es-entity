use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{list_by_fn::CursorStruct, options::*};

pub struct ListForFn<'a> {
    ignore_prefix: Option<&'a syn::LitStr>,
    pub for_column: &'a Column,
    pub by_column: &'a Column,
    entity: &'a syn::Ident,
    id: &'a syn::Ident,
    table_name: &'a str,
    error: &'a syn::Type,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    any_nested: bool,
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
            error: opts.err(),
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            any_nested: opts.any_nested(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    #[cfg(test)]
    pub fn new_test(
        for_column: &'a Column,
        by_column: &'a Column,
        entity: &'a syn::Ident,
        id: &'a syn::Ident,
        table_name: &'a str,
        error: &'a syn::Type,
        cursor_mod: syn::Ident,
    ) -> Self {
        Self {
            ignore_prefix: None,
            for_column,
            by_column,
            entity,
            id,
            table_name,
            error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
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
}

impl ToTokens for ListForFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let error = self.error;
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
        let arg_tokens = cursor.query_arg_tokens();

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

            let asc_query = format!(
                r#"SELECT {} FROM {} WHERE (({} = $1) AND ({})){} ORDER BY {} LIMIT $2"#,
                select_columns,
                self.table_name,
                for_column_name,
                cursor.condition(1, true),
                if delete == DeleteOption::No {
                    self.delete.not_deleted_condition()
                } else {
                    ""
                },
                cursor.order_by(true)
            );
            let desc_query = format!(
                r#"SELECT {} FROM {} WHERE (({} = $1) AND ({})){} ORDER BY {} LIMIT $2"#,
                select_columns,
                self.table_name,
                for_column_name,
                cursor.condition(1, false),
                if delete == DeleteOption::No {
                    self.delete.not_deleted_condition()
                } else {
                    ""
                },
                cursor.order_by(false)
            );

            let es_query_asc_call = if let Some(prefix) = self.ignore_prefix {
                quote! {
                    es_entity::es_query!(
                        tbl_prefix = #prefix,
                        #asc_query,
                        #filter_arg_name as &#for_column_type,
                        #arg_tokens
                    )
                }
            } else {
                quote! {
                    es_entity::es_query!(
                        entity = #entity,
                        #asc_query,
                        #filter_arg_name as &#for_column_type,
                        #arg_tokens
                    )
                }
            };

            let es_query_desc_call = if let Some(prefix) = self.ignore_prefix {
                quote! {
                    es_entity::es_query!(
                        tbl_prefix = #prefix,
                        #desc_query,
                        #filter_arg_name as &#for_column_type,
                        #arg_tokens
                    )
                }
            } else {
                quote! {
                    es_entity::es_query!(
                        entity = #entity,
                        #desc_query,
                        #filter_arg_name as &#for_column_type,
                        #arg_tokens
                    )
                }
            };

            #[cfg(feature = "instrument")]
            let (instrument_attr, extract_has_cursor, record_fields, record_results) = {
                let entity_name = entity.to_string();
                let repo_name = &self.repo_name_snake;
                let span_name = format!(
                    "{}.list_for_{}_by_{}",
                    repo_name, for_column_name, by_column_name
                );
                let filter_field_name = format!("query_{}", filter_arg_name);
                let filter_field_ident = syn::Ident::new(&filter_field_name, proc_macro2::Span::call_site());
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #filter_field_ident = tracing::field::Empty, first, has_cursor, direction = tracing::field::debug(&direction), count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty), err(level = "warn"))]
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
                )
            };
            #[cfg(not(feature = "instrument"))]
            let (instrument_attr, extract_has_cursor, record_fields, record_results) =
                (quote! {}, quote! {}, quote! {}, quote! {});

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    #filter_arg_name: #for_impl_expr,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #error> {
                    self.#fn_in_op(#query_fn_get_op, #filter_arg_name, cursor, direction).await
                }

                #instrument_attr
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
                    #extract_has_cursor
                    let #filter_arg_name = #filter_arg_name.#for_access_expr;
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
    fn list_for_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
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
            error: &error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_customer_id_by_id(
                &self,
                filter_customer_id: impl std::borrow::Borrow<Uuid>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntitiesByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntitiesByIdCursor>, es_entity::EsRepoError> {
                self.list_for_customer_id_by_id_in_op(self.pool(), filter_customer_id, cursor, direction).await
            }

            pub async fn list_for_customer_id_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                filter_customer_id: impl std::borrow::Borrow<Uuid>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntitiesByIdCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntitiesByIdCursor>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
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
                            "SELECT customer_id, id FROM entities WHERE ((customer_id = $1) AND (COALESCE(id > $3, true))) ORDER BY id ASC LIMIT $2",
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
                            "SELECT customer_id, id FROM entities WHERE ((customer_id = $1) AND (COALESCE(id < $3, true))) ORDER BY id DESC LIMIT $2",
                            filter_customer_id as &Uuid,
                            (first + 1) as i64,
                            id as Option<EntityId>,
                        )
                            .fetch_n(op, first)
                            .await?
                    }
                };

                    let end_cursor = entities.last().map(cursor_mod::EntitiesByIdCursor::from);
                    Ok(es_entity::PaginatedQueryRet {
                        entities,
                        has_next_page,
                        end_cursor,
                    })
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_same_column() {
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
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
            error: &error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn list_for_email_by_email(
                &self,
                filter_email: impl std::convert::AsRef<str>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntitiesByEmailCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntitiesByEmailCursor>, es_entity::EsRepoError> {
                self.list_for_email_by_email_in_op(self.pool(), filter_email, cursor, direction).await
            }

            pub async fn list_for_email_by_email_in_op<'a, OP>(
                &self,
                op: OP,
                filter_email: impl std::convert::AsRef<str>,
                cursor: es_entity::PaginatedQueryArgs<cursor_mod::EntitiesByEmailCursor>,
                direction: es_entity::ListDirection,
            ) -> Result<es_entity::PaginatedQueryRet<Entity, cursor_mod::EntitiesByEmailCursor>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
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
                            "SELECT email, id FROM entities WHERE ((email = $1) AND (COALESCE((email, id) > ($4, $3), $3 IS NULL))) ORDER BY email ASC, id ASC LIMIT $2",
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
                            "SELECT email, id FROM entities WHERE ((email = $1) AND (COALESCE((email, id) < ($4, $3), $3 IS NULL))) ORDER BY email DESC, id DESC LIMIT $2",
                            filter_email as &str,
                            (first + 1) as i64,
                            id as Option<EntityId>,
                            email as Option<String>,
                        )
                            .fetch_n(op, first)
                            .await?
                    }
                };

                let end_cursor = entities.last().map(cursor_mod::EntitiesByEmailCursor::from);
                Ok(es_entity::PaginatedQueryRet {
                    entities,
                    has_next_page,
                    end_cursor,
                })
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
