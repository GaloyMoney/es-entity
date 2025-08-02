use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::RepositoryOptions;

pub struct PostPersistHook<'a> {
    event: &'a syn::Ident,
    entity: &'a syn::Ident,
    error: &'a syn::Type,
    hook: &'a Option<syn::Ident>,
    additional_op_constraint: proc_macro2::TokenStream,
}

impl<'a> From<&'a RepositoryOptions> for PostPersistHook<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            event: opts.event(),
            entity: opts.entity(),
            error: opts.err(),
            hook: &opts.post_persist_hook,
            additional_op_constraint: opts.additional_op_constraint(),
        }
    }
}

impl ToTokens for PostPersistHook<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let event = &self.event;
        let entity = &self.entity;
        let error = &self.error;
        let additional_op_constraint = &self.additional_op_constraint;

        let hook = if let Some(hook) = self.hook {
            quote! {
                self.#hook(op, entity, new_events).await?;
                Ok(())
            }
        } else {
            quote! {
                Ok(())
            }
        };

        tokens.append_all(quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(
                &self,
                op: &mut OP,
                entity: &#entity,
                new_events: es_entity::LastPersisted<'_, #event>
            ) -> Result<(), #error>
                where
                    OP: for<'o> es_entity::AtomicOperation<'o>
                    #additional_op_constraint
            {
                #hook
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_persist_hook() {
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let hook = None;

        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            error: &error,
            hook: &hook,
            additional_op_constraint: quote! {},
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, #event>
            ) -> Result<(), es_entity::EsRepoError>
                where
                    OP: for<'o> es_entity::AtomicOperation<'o>
            {
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn post_persist_hook_with_additional_traits() {
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();
        let hook = None;
        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            error: &error,
            hook: &hook,
            additional_op_constraint: quote! { , OP: Send + Sync },
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, #event>
            ) -> Result<(), es_entity::EsRepoError>
                where
                    OP: for<'o> es_entity::AtomicOperation<'o>
                    , OP: Send + Sync
            {
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
