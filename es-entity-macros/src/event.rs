use convert_case::{Case, Casing};
use darling::{FromDeriveInput, ToTokens};
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

#[derive(Debug, Clone, FromDeriveInput)]
#[darling(attributes(es_event))]
pub struct EsEvent {
    ident: syn::Ident,
    data: darling::ast::Data<syn::Variant, ()>,
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

        let match_arms = match &self.data {
            darling::ast::Data::Enum(variants) => {
                let arms: Vec<_> = variants
                    .iter()
                    .map(|v| {
                        let variant_ident = &v.ident;
                        let snake_name = variant_ident.to_string().to_case(Case::Snake);
                        quote! {
                            Self::#variant_ident { .. } => #snake_name,
                        }
                    })
                    .collect();
                quote! { #(#arms)* }
            }
            _ => panic!("EsEvent can only be derived for enums"),
        };

        tokens.append_all(quote! {
            impl es_entity::EsEvent for #ident {
                type EntityId = #id;

                fn event_context() -> bool {
                    #event_context
                }

                fn event_type(&self) -> &'static str {
                    match self {
                        #match_arms
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_event_type_match() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[es_event(id = "UserId")]
            enum UserEvent {
                Initialized { id: UserId, name: String },
                NameUpdated { name: String },
                Deactivated { reason: String },
                AccountClosed {},
            }
        };
        let event = EsEvent::from_derive_input(&input).unwrap();
        let mut tokens = TokenStream::new();
        event.to_tokens(&mut tokens);

        let expected = quote! {
            impl es_entity::EsEvent for UserEvent {
                type EntityId = UserId;

                fn event_context() -> bool {
                    false
                }

                fn event_type(&self) -> &'static str {
                    match self {
                        Self::Initialized { .. } => "initialized",
                        Self::NameUpdated { .. } => "name_updated",
                        Self::Deactivated { .. } => "deactivated",
                        Self::AccountClosed { .. } => "account_closed",
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
