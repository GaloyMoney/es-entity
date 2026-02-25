use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct FindByFn<'a> {
    prefix: Option<&'a syn::LitStr>,
    entity: &'a syn::Ident,
    column: &'a Column,
    table_name: &'a str,
    error: &'a syn::Type,
    delete: DeleteOption,
    any_nested: bool,
    include_deleted_queries: bool,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> FindByFn<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            prefix: opts.table_prefix(),
            column,
            entity: opts.entity(),
            table_name: opts.table_name(),
            error: opts.err(),
            delete: opts.delete,
            any_nested: opts.any_nested(),
            include_deleted_queries: opts.include_deleted_queries,
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for FindByFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let column_name = &self.column.name();
        let (column_type, impl_expr, access_expr) = &self.column.ty_for_find_by();
        let error = self.error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        for maybe in ["", "maybe_"] {
            let (result_type, fetch_fn) = if maybe.is_empty() {
                (quote! { #entity }, quote! { fetch_one(op) })
            } else {
                (quote! { Option<#entity> }, quote! { fetch_optional(op) })
            };

            for delete in [DeleteOption::No, DeleteOption::Soft] {
                let fn_name = syn::Ident::new(
                    &format!(
                        "{}find_by_{}{}",
                        maybe,
                        column_name,
                        delete.include_deletion_fn_postfix()
                    ),
                    Span::call_site(),
                );
                let fn_in_op = syn::Ident::new(
                    &format!(
                        "{}find_by_{}{}_in_op",
                        maybe,
                        column_name,
                        delete.include_deletion_fn_postfix()
                    ),
                    Span::call_site(),
                );

                let query = format!(
                    r#"SELECT id FROM {} WHERE {} = $1{}"#,
                    self.table_name,
                    column_name,
                    if delete == DeleteOption::No {
                        self.delete.not_deleted_condition()
                    } else {
                        ""
                    }
                );

                let es_query_call = if let Some(prefix) = self.prefix {
                    quote! {
                        es_entity::es_query!(
                            tbl_prefix = #prefix,
                            #query,
                            #column_name as &#column_type,
                        )
                    }
                } else {
                    quote! {
                        es_entity::es_query!(
                            entity = #entity,
                            #query,
                            #column_name as &#column_type,
                        )
                    }
                };

                #[cfg(feature = "instrument")]
                let (instrument_attr_in_op, record_field, error_recording) = {
                    let entity_name = entity.to_string();
                    let repo_name = &self.repo_name_snake;
                    let span_name = format!("{}.{}find_by_{}", repo_name, maybe, column_name);
                    let field_name = format!("query_{}", column_name);
                    let field_ident = syn::Ident::new(&field_name, proc_macro2::Span::call_site());
                    (
                        quote! {
                            #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #field_ident = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                        },
                        quote! {
                            tracing::Span::current().record(#field_name, tracing::field::debug(&#column_name));
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
                let (instrument_attr_in_op, record_field, error_recording) =
                    (quote! {}, quote! {}, quote! {});

                tokens.append_all(quote! {
                    pub async fn #fn_name(
                        &self,
                        #column_name: #impl_expr
                    ) -> Result<#result_type, #error> {
                        self.#fn_in_op(#query_fn_get_op, #column_name).await
                    }

                    #instrument_attr_in_op
                    pub async fn #fn_in_op #query_fn_generics(
                        &self,
                        #query_fn_op_arg,
                        #column_name: #impl_expr
                    ) -> Result<#result_type, #error>
                        where
                            OP: #query_fn_op_traits
                    {
                        let __result: Result<#result_type, #error> = async {
                            let #column_name = #column_name.#access_expr;
                            #record_field
                            #es_query_call.#fetch_fn.await
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn find_by_fn() {
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindByFn {
            prefix: None,
            column: &column,
            entity: &entity,
            table_name: "entities",
            error: &error,
            delete: DeleteOption::No,
            any_nested: false,
            include_deleted_queries: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError> {
                self.find_by_id_in_op(self.pool(), id).await
            }

            pub async fn find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await
                }.await;

                __result
            }

            pub async fn maybe_find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError> {
                self.maybe_find_by_id_in_op(self.pool(), id).await
            }

            pub async fn maybe_find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Option<Entity>, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn find_by_fn_string_arg() {
        let column = Column::new(
            syn::Ident::new("email", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindByFn {
            prefix: None,
            column: &column,
            entity: &entity,
            table_name: "entities",
            error: &error,
            delete: DeleteOption::No,
            any_nested: false,
            include_deleted_queries: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_by_email(
                &self,
                email: impl std::convert::AsRef<str>
            ) -> Result<Entity, es_entity::EsRepoError> {
                self.find_by_email_in_op(self.pool(), email).await
            }

            pub async fn find_by_email_in_op<'a, OP>(
                &self,
                op: OP,
                email: impl std::convert::AsRef<str>
            ) -> Result<Entity, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let email = email.as_ref();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE email = $1",
                        email as &str,
                    )
                    .fetch_one(op)
                    .await
                }.await;

                __result
            }

            pub async fn maybe_find_by_email(
                &self,
                email: impl std::convert::AsRef<str>
            ) -> Result<Option<Entity>, es_entity::EsRepoError> {
                self.maybe_find_by_email_in_op(self.pool(), email).await
            }

            pub async fn maybe_find_by_email_in_op<'a, OP>(
                &self,
                op: OP,
                email: impl std::convert::AsRef<str>
            ) -> Result<Option<Entity>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Option<Entity>, es_entity::EsRepoError> = async {
                    let email = email.as_ref();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE email = $1",
                        email as &str,
                    )
                    .fetch_optional(op)
                    .await
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn find_by_fn_with_soft_delete() {
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindByFn {
            prefix: None,
            column: &column,
            entity: &entity,
            table_name: "entities",
            error: &error,
            delete: DeleteOption::Soft,
            any_nested: false,
            include_deleted_queries: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError> {
                self.find_by_id_in_op(self.pool(), id).await
            }

            pub async fn find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1 AND deleted = FALSE",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await
                }.await;

                __result
            }

            pub async fn maybe_find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError> {
                self.maybe_find_by_id_in_op(self.pool(), id).await
            }

            pub async fn maybe_find_by_id_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError>
                where
                    OP: es_entity::IntoOneTimeExecutor<'a>
            {
                let __result: Result<Option<Entity>, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1 AND deleted = FALSE",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn find_by_fn_with_soft_delete_include_deleted() {
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindByFn {
            prefix: None,
            column: &column,
            entity: &entity,
            table_name: "entities",
            error: &error,
            delete: DeleteOption::Soft,
            any_nested: false,
            include_deleted_queries: true,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();
        assert!(token_str.contains("find_by_id_include_deleted"));
        assert!(token_str.contains("maybe_find_by_id_include_deleted"));
    }

    #[test]
    fn find_by_fn_nested() {
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindByFn {
            prefix: None,
            column: &column,
            entity: &entity,
            table_name: "entities",
            error: &error,
            delete: DeleteOption::No,
            any_nested: true,
            include_deleted_queries: false,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError> {
                self.find_by_id_in_op(&mut self.pool().begin().await?, id).await
            }

            pub async fn find_by_id_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError>
                where
                    OP: es_entity::AtomicOperation
            {
                let __result: Result<Entity, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await
                }.await;

                __result
            }

            pub async fn maybe_find_by_id(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError> {
                self.maybe_find_by_id_in_op(&mut self.pool().begin().await?, id).await
            }

            pub async fn maybe_find_by_id_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError>
                where
                    OP: es_entity::AtomicOperation
            {
                let __result: Result<Option<Entity>, es_entity::EsRepoError> = async {
                    let id = id.borrow();
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await
                }.await;

                __result
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
