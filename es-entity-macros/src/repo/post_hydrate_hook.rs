use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::{PostHydrateHookConfig, RepositoryOptions};

pub struct PostHydrateHook<'a> {
    entity: &'a syn::Ident,
    hook: &'a Option<PostHydrateHookConfig>,
}

impl<'a> From<&'a RepositoryOptions> for PostHydrateHook<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            entity: opts.entity(),
            hook: &opts.post_hydrate_hook,
        }
    }
}

impl ToTokens for PostHydrateHook<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = &self.entity;

        let (return_type, hook) = if let Some(config) = self.hook {
            let method = &config.method;
            let error_ty = &config.error;
            (
                quote! { #error_ty },
                quote! {
                    self.#method(entity)
                },
            )
        } else {
            (
                quote! { std::convert::Infallible },
                quote! {
                    Ok(())
                },
            )
        };

        tokens.append_all(quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &#entity,
            ) -> Result<(), #return_type> {
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
        let hook = None;

        let hook = PostHydrateHook {
            entity: &entity,
            hook: &hook,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &Entity,
            ) -> Result<(), std::convert::Infallible> {
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn post_hydrate_hook_some() {
        let entity = syn::Ident::new("Entity", proc_macro2::Span::call_site());
        let hook = Some(PostHydrateHookConfig {
            method: syn::Ident::new("validate_entity", proc_macro2::Span::call_site()),
            error: syn::parse_str("EntityPostHydrateError").unwrap(),
        });

        let hook = PostHydrateHook {
            entity: &entity,
            hook: &hook,
        };

        let mut tokens = TokenStream::new();
        hook.to_tokens(&mut tokens);

        let expected = quote! {
            #[inline(always)]
            fn execute_post_hydrate_hook(
                &self,
                entity: &Entity,
            ) -> Result<(), EntityPostHydrateError> {
                self.validate_entity(entity)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
