use darling::{FromDeriveInput, ToTokens};
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

#[derive(Debug, Clone, FromDeriveInput)]
#[darling(attributes(es_event))]
pub struct EsEvent {
    ident: syn::Ident,
    id: syn::Type,
    #[darling(default, rename = "event_context")]
    event_ctx: Option<bool>,
}

pub fn derive(ast: syn::DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let event = EsEvent::from_derive_input(&ast)?;
    Ok(quote!(#event))
}

impl ToTokens for EsEvent {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = &self.ident;
        let id = &self.id;
        let event_context = {
            #[cfg(feature = "event-context")]
            {
                self.event_ctx.unwrap_or(true)
            }
            #[cfg(not(feature = "event-context"))]
            {
                self.event_ctx.unwrap_or(false)
            }
        };
        tokens.append_all(quote! {
            impl es_entity::EsEvent for #ident {
                type EntityId = #id;

                fn event_context() -> bool {
                    #event_context
                }
            }
        });
    }
}
