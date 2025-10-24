use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

#[cfg(feature = "instrument")]
use convert_case::{Case, Casing};

use super::options::*;

pub struct FindAllFn<'a> {
    prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    table_name: &'a str,
    error: &'a syn::Type,
    any_nested: bool,
}

impl<'a> From<&'a RepositoryOptions> for FindAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            prefix: opts.table_prefix(),
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            error: opts.err(),
            any_nested: opts.any_nested(),
        }
    }
}

impl ToTokens for FindAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id = self.id;
        let entity = self.entity;
        let error = self.error;
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        let generics = if self.any_nested {
            quote! { <Out: From<#entity>> }
        } else {
            quote! { <'a, Out: From<#entity>> }
        };

        let query = format!("SELECT id FROM {} WHERE id = ANY($1)", self.table_name);

        let es_query_call = if let Some(prefix) = self.prefix {
            quote! {
                es_entity::es_query!(
                    tbl_prefix = #prefix,
                    #query,
                    ids as &[#id],
                )
            }
        } else {
            quote! {
                es_entity::es_query!(
                    entity = #entity,
                    #query,
                    ids as &[#id],
                )
            }
        };

        let op_param = if self.any_nested {
            quote! { op: &mut impl #query_fn_op_traits }
        } else {
            quote! { op: impl #query_fn_op_traits }
        };

        #[cfg(feature = "instrument")]
        let instrument_attr = {
            let entity_name = entity.to_string();
            let span_name = format!("es.{}.find_all", entity_name.to_case(Case::Snake));
            quote! {
                #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = ids.len(), ids = tracing::field::debug(ids)), err(level = "warn"))]
            }
        };
        #[cfg(not(feature = "instrument"))]
        let instrument_attr = quote! {};

        tokens.append_all(quote! {
            pub async fn find_all<Out: From<#entity>>(
                &self,
                ids: &[#id]
            ) -> Result<std::collections::HashMap<#id, Out>, #error> {
                self.find_all_in_op(#query_fn_get_op, ids).await
            }

            #instrument_attr
            pub async fn find_all_in_op #generics(
                &self,
                #op_param,
                ids: &[#id]
            ) -> Result<std::collections::HashMap<#id, Out>, #error> {
                 let (entities, _) = #es_query_call.fetch_n(op, ids.len()).await?;
                 Ok(entities.into_iter().map(|u| (u.id.clone(), Out::from(u))).collect())
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn find_all_fn() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let persist_fn = FindAllFn {
            prefix: None,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            error: &error,
            any_nested: false,
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn find_all<Out: From<Entity>>(
                &self,
                ids: &[EntityId]
            ) -> Result<std::collections::HashMap<EntityId, Out>, es_entity::EsRepoError> {
                self.find_all_in_op(self.pool(), ids).await
            }

            pub async fn find_all_in_op<'a, Out: From<Entity>>(
                &self,
                op: impl es_entity::IntoOneTimeExecutor<'a>,
                ids: &[EntityId]
            ) -> Result<std::collections::HashMap<EntityId, Out>, es_entity::EsRepoError> {
                let (entities, _) = es_entity::es_query!(
                    entity = Entity,
                    "SELECT id FROM entities WHERE id = ANY($1)",
                    ids as &[EntityId],
                )
                    .fetch_n(op, ids.len())
                    .await?;
                Ok(entities.into_iter().map(|u| (u.id.clone(), Out::from(u))).collect())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
