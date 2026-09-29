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

/// Information about forgettable fields in an event enum.
struct ForgettableInfo {
    /// Whether any variant has forgettable fields.
    has_forgettable: bool,
    /// Per-variant: (variant_ident, list_of_forgettable_field_idents)
    variants: Vec<(syn::Ident, Vec<syn::Ident>)>,
}

pub fn derive(ast: syn::DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let event = EsEvent::from_derive_input(&ast)?;
    let forgettable_info = extract_forgettable_info(&ast);
    let ident = &event.ident;

    let mut tokens = quote!(#event);

    // Generate forgettable support methods
    let has_forgettable = forgettable_info.has_forgettable;

    let match_arms: Vec<_> = forgettable_info
        .variants
        .iter()
        .map(|(variant_ident, field_idents)| {
            if field_idents.is_empty() {
                quote! {
                    #ident::#variant_ident { .. } => None,
                }
            } else {
                let field_name_strs: Vec<String> =
                    field_idents.iter().map(|i| i.to_string()).collect();
                let inserts: Vec<_> = field_idents
                    .iter()
                    .zip(field_name_strs.iter())
                    .map(|(field_id, field_name)| {
                        quote! {
                            if let Some(v) = #field_id.__extract_payload_value() {
                                payload.insert(
                                    #field_name.to_string(),
                                    v,
                                );
                            }
                        }
                    })
                    .collect();
                quote! {
                    #ident::#variant_ident { #(#field_idents),*, .. } => {
                        let mut payload = es_entity::prelude::serde_json::Map::new();
                        #(#inserts)*
                        if payload.is_empty() { None } else { Some(payload.into()) }
                    }
                }
            }
        })
        .collect();

    let forget_match_arms: Vec<_> = forgettable_info
        .variants
        .iter()
        .map(|(variant_ident, field_idents)| {
            if field_idents.is_empty() {
                quote! {
                    #ident::#variant_ident { .. } => {}
                }
            } else {
                let assignments: Vec<_> = field_idents
                    .iter()
                    .map(|field_id| {
                        quote! {
                            *#field_id = es_entity::Forgettable::forgotten();
                        }
                    })
                    .collect();
                quote! {
                    #ident::#variant_ident { #(#field_idents),*, .. } => {
                        #(#assignments)*
                    }
                }
            }
        })
        .collect();

    tokens.append_all(quote! {
        impl #ident {
            #[doc(hidden)]
            pub const HAS_FORGETTABLE_FIELDS: bool = #has_forgettable;

            #[doc(hidden)]
            pub fn extract_forgettable_payloads(&self) -> Option<es_entity::prelude::serde_json::Value> {
                match self {
                    #(#match_arms)*
                }
            }

            #[doc(hidden)]
            pub fn forget_forgettable_payloads(&mut self) {
                match self {
                    #(#forget_match_arms)*
                }
            }
        }
    });

    Ok(tokens)
}

/// The value written to the `event_type` column for a variant.
///
/// Deliberately the snake-cased *ident*, not the serde tag: the column is
/// populated from the generated `EsEvent::event_type`, which knows nothing
/// about serde renames, so anything matching against that column has to agree
/// with it.
fn event_type_value(variant_ident: &syn::Ident) -> String {
    variant_ident.to_string().to_case(Case::Snake)
}

/// Extract forgettable field information from the enum definition.
fn extract_forgettable_info(ast: &syn::DeriveInput) -> ForgettableInfo {
    let variants = match &ast.data {
        syn::Data::Enum(data) => data
            .variants
            .iter()
            .map(|variant| {
                let variant_ident = variant.ident.clone();
                let forgettable_fields = variant
                    .fields
                    .iter()
                    .filter_map(|field| {
                        if crate::type_utils::is_forgettable_type(&field.ty) {
                            field.ident.clone()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                (variant_ident, forgettable_fields)
            })
            .collect(),
        _ => Vec::new(),
    };

    let has_forgettable = variants.iter().any(|(_, fields)| !fields.is_empty());

    ForgettableInfo {
        has_forgettable,
        variants,
    }
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
                        let type_value = event_type_value(variant_ident);
                        quote! {
                            Self::#variant_ident { .. } => #type_value,
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
