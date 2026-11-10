use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::RepositoryOptions;

/// The hook runs queries through `op`, so it can fail transiently as well as
/// fatally: it returns `Fault<lanes!(Transient, Fatal)>`, which the calling op
/// `?`s into its own `RepoWriteError`.
pub struct PostPersistHook<'a> {
    event: &'a syn::Ident,
    entity: &'a syn::Ident,
    hook: Option<&'a syn::Ident>,
}

impl<'a> From<&'a RepositoryOptions> for PostPersistHook<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            event: opts.event(),
            entity: opts.entity(),
            hook: opts.post_persist_hook.as_ref(),
        }
    }
}

impl ToTokens for PostPersistHook<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let event = &self.event;
        let entity = &self.entity;

        let (hook, op_param) = match self.hook {
            Some(method) => (
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
                    self.#method(&mut op, entity, new_events).await
                },
                // `mut` is only needed to take `&mut op` above; declaring it
                // unconditionally would warn `unused_mut` on the no-hook path.
                quote! { mut op: &mut OP },
            ),
            None => (quote! { Ok(()) }, quote! { op: &mut OP }),
        };

        tokens.append_all(quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(
                &self,
                #op_param,
                entity: &#entity,
                new_events: es_entity::LastPersisted<'_, #event>
            ) -> Result<(), errlanes::Fault<errlanes::lanes!(Transient, Fatal)>>
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

        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            hook: None,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, EntityEvent>
            ) -> Result<(), errlanes::Fault<errlanes::lanes!(Transient, Fatal)>>
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
        let method = syn::Ident::new("on_persist", proc_macro2::Span::call_site());

        let hook = PostPersistHook {
            event: &event,
            entity: &entity,
            hook: Some(&method),
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            async fn execute_post_persist_hook<OP>(&self,
                mut op: &mut OP,
                entity: &Entity,
                new_events: es_entity::LastPersisted<'_, EntityEvent>
            ) -> Result<(), errlanes::Fault<errlanes::lanes!(Transient, Fatal)>>
                where
                    OP: es_entity::AtomicOperation + ?Sized
            {
                self.on_persist(&mut op, entity, new_events).await
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
