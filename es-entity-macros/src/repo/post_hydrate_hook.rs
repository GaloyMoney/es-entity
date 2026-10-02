use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::RepositoryOptions;

/// The hook is synchronous and has no `op`, so it can never see a retryable
/// failure: it returns a bare `Fatal`, which the calling op `?`s into its own
/// `RepoFault`/`RepoWriteError`.
pub struct PostHydrateHook<'a> {
    entity: &'a syn::Ident,
    hook: Option<&'a syn::Ident>,
}

impl<'a> From<&'a RepositoryOptions> for PostHydrateHook<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            hook: opts.post_hydrate_hook.as_ref(),
        }
    }
}

impl ToTokens for PostHydrateHook<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = &self.entity;

        let hook = match self.hook {
            Some(method) => quote! { self.#method(entity) },
            None => quote! { Ok(()) },
        };

        tokens.append_all(quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &#entity,
            ) -> Result<(), errlanes::Fatal> {
                #hook
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_hydrate_hook_none() {
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());

        let hook = PostHydrateHook {
            entity: &entity,
            hook: None,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &Entity,
            ) -> Result<(), errlanes::Fatal> {
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn post_hydrate_hook_some() {
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let method = syn::Ident::new("validate_entity", proc_macro2::Span::call_site());

        let hook = PostHydrateHook {
            entity: &entity,
            hook: Some(&method),
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &Entity,
            ) -> Result<(), errlanes::Fatal> {
                self.validate_entity(entity)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
