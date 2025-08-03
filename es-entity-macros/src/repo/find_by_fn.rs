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
        }
    }
}

impl ToTokens for FindByFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let column_name = &self.column.name();
        let column_type = &self.column.ty_for_find_by();
        let error = self.error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);
        for maybe in ["", "maybe_"] {
            let (result_type, fetch_fn) = if maybe.is_empty() {
                (
                    quote! { #entity },
                    if self.any_nested {
                        quote! { .fetch_one_include_nested(op) }
                    } else {
                        quote! { .fetch_one(op) }
                    },
                )
            } else {
                (
                    quote! { Option<#entity> },
                    if self.any_nested {
                        quote! { .fetch_optional_include_nested(op) }
                    } else {
                        quote! { .fetch_optional(op) }
                    },
                )
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

                tokens.append_all(quote! {
                    pub async fn #fn_name(
                        &self,
                        #column_name: impl std::borrow::Borrow<#column_type>
                    ) -> Result<#result_type, #error> {
                        self.#fn_in_op(#query_fn_get_op, #column_name).await
                    }

                    pub async fn #fn_in_op #query_fn_generics(
                        &self,
                        #query_fn_op_arg,
                        #column_name: impl std::borrow::Borrow<#column_type>
                    ) -> Result<#result_type, #error>
                        where
                            OP: #query_fn_op_traits
                    {
                        let #column_name = #column_name.borrow();
                        Ok(
                            #es_query_call
                            #fetch_fn
                            .await?
                        )
                    }
                });

                if delete == self.delete {
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
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await?
                )
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
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await?
                )
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
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1 AND deleted = FALSE",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await?
                )
            }

            pub async fn find_by_id_include_deleted(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError> {
                self.find_by_id_include_deleted_in_op(self.pool(), id).await
            }

            pub async fn find_by_id_include_deleted_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Entity, es_entity::EsRepoError>
                where
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_one(op)
                    .await?
                )
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
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1 AND deleted = FALSE",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await?
                )
            }

            pub async fn maybe_find_by_id_include_deleted(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError> {
                self.maybe_find_by_id_include_deleted_in_op(self.pool(), id).await
            }

            pub async fn maybe_find_by_id_include_deleted_in_op<'a, OP>(
                &self,
                op: OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<Option<Entity>, es_entity::EsRepoError>
                where
                    OP: IntoExecutor<'a>
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_optional(op)
                    .await?
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
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
                    OP: AtomicOperation
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_one_include_nested(op)
                    .await?
                )
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
                    OP: AtomicOperation
            {
                let id = id.borrow();
                Ok(
                    es_entity::es_query!(
                        entity = Entity,
                        "SELECT id FROM entities WHERE id = $1",
                        id as &EntityId,
                    )
                    .fetch_optional_include_nested(op)
                    .await?
                )
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
