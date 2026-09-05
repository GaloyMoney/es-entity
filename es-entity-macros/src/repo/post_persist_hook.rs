use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::RepositoryOptions;
use super::options::PostPersistHookConfig;

pub struct PostPersistHook<'a> {
    event: &'a syn::Ident,
    entity: &'a syn::Ident,
    hook: &'a Option<PostPersistHookConfig>,
}

impl<'a> From<&'a RepositoryOptions> for PostPersistHook<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            event: opts.event(),
            entity: opts.entity(),
            hook: &opts.post_persist_hook,
        }
    }
}

impl ToTokens for PostPersistHook<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let event = &self.event;
        let entity = &self.entity;

        let (error_ty, hook, op_param) = if let Some(config) = self.hook {
            let method = &config.method;
            let error = &config.error;
            (
                quote! { #error },
                quote! {
                    // The caller's hook method (`#method`) lives in the
                    // consuming crate and, by every existing convention, is
                    // generic over a plain (implicitly `Sized`) `impl
                    // AtomicOperation` — its signature is out of this macro's
                    // control and cannot be forced to add `?Sized`.
                    //
                    // Reborrowing through one more `&mut` sidesteps that: a
                    // `&mut OP` is always `Sized` regardless of whether `OP`
                    // itself is, and it implements `AtomicOperation` via the
                    // blanket `impl<O: AtomicOperation + ?Sized> AtomicOperation
                    // for &mut O`. So `&mut op` satisfies the hook method's
                    // `Sized` bound no matter what `OP` is here, letting this
                    // wrapper — and therefore every caller of it — stay
                    // `?Sized` unconditionally.
                    self.#method(&mut op, entity, new_events).await?;
                    Ok(())
                },
                // `mut` is only needed to take `&mut op` above; declaring it
                // unconditionally would warn `unused_mut` on the no-hook path.
                quote! { mut op: &mut OP },
            )
        } else {
            (
                quote! { sqlx::Error },
                quote! {
                    Ok(())
                },
                quote! { op: &mut OP },
            )
        };

        tokens.append_all(quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(
                &self,
                #op_param,
                entity: &#entity,
                new_events: es_entity::LastPersisted<'_, #event>
            ) -> Result<(), #error_ty>
                where
                    OP: es_entity::AtomicOperation + ?Sized
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
    fn post_persist_hook_none() {
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let hook = None;

        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            hook: &hook,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, EntityEvent>
            ) -> Result<(), sqlx::Error>
                where
                    OP: es_entity::AtomicOperation + ?Sized
            {
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn post_persist_hook_some() {
        let event = syn::Ident::new("EntityEvent", proc_macro2::Span::call_site());
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let config = Some(PostPersistHookConfig {
            method: syn::Ident::new("on_persist", proc_macro2::Span::call_site()),
            error: syn::parse_str("MyPersistError").unwrap(),
        });

        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            hook: &config,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                mut op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, EntityEvent>
            ) -> Result<(), MyPersistError>
                where
                    OP: es_entity::AtomicOperation + ?Sized
            {
                self.on_persist(&mut op, entity, new_events).await?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
