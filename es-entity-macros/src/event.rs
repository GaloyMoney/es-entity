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
    /// Per-variant: (variant_ident, event_type_value, list_of_forgettable_field_idents)
    variants: Vec<(syn::Ident, String, Vec<syn::Ident>)>,
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
        .map(|(variant_ident, _tag_value, field_idents)| {
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
        .map(|(variant_ident, _tag_value, field_idents)| {
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

    // (event_type column value, forgettable field JSON key) pairs across all
    // variants — consumed by the repo's generated `verify_forgotten` storage
    // check, which joins the first element against the `event_type` column.
    let forgettable_json_fields: Vec<_> = forgettable_info
        .variants
        .iter()
        .flat_map(|(_, event_type_value, field_idents)| {
            field_idents.iter().map(move |field_id| {
                let field_name = field_id.to_string();
                quote! { (#event_type_value, #field_name) }
            })
        })
        .collect();

    tokens.append_all(quote! {
        impl #ident {
            #[doc(hidden)]
            pub const HAS_FORGETTABLE_FIELDS: bool = #has_forgettable;

            #[doc(hidden)]
            pub const FORGETTABLE_JSON_FIELDS: &'static [(&'static str, &'static str)] = &[
                #(#forgettable_json_fields),*
            ];

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
/// with it. Both emission sites go through here — the `event_type` arms and
/// `FORGETTABLE_JSON_FIELDS` — so the two cannot drift apart.
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
                let event_type_value = event_type_value(&variant_ident);
                let forgettable_fields = variant
                    .fields
                    .iter()
                    .filter_map(|field| {
                        if is_forgettable_type(&field.ty) {
                            field.ident.clone()
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                (variant_ident, event_type_value, forgettable_fields)
            })
            .collect(),
        _ => Vec::new(),
    };

    let has_forgettable = variants.iter().any(|(_, _, fields)| !fields.is_empty());

    ForgettableInfo {
        has_forgettable,
        variants,
    }
}

/// Check if a type's last path segment is "Forgettable".
fn is_forgettable_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "Forgettable";
    }
    false
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

        // With no `event_context` attribute the default depends on the
        // `event-context` feature
        let event_context = cfg!(feature = "event-context");
        let expected = quote! {
            impl es_entity::EsEvent for UserEvent {
                type EntityId = UserId;

                fn event_context() -> bool {
                    #event_context
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

    /// `verify_forgotten` joins `FORGETTABLE_JSON_FIELDS` against the
    /// `event_type` column, which is written from `event_type()` — the
    /// snake-cased ident, which knows nothing about serde renames. Keying
    /// the const on the serde tag instead would make the join match nothing
    /// and silently report a forgotten entity as clean. Every entity in this
    /// repo happens to use `rename_all = "snake_case"`, where the two agree,
    /// so this pins the case where they do not.
    #[test]
    fn forgettable_json_fields_key_on_the_event_type_column_not_the_serde_tag() {
        let input: syn::DeriveInput = syn::parse_quote! {
            #[es_event(id = "SubscriberId")]
            #[serde(tag = "type", rename_all = "camelCase")]
            enum SubscriberEvent {
                #[serde(rename = "totally_custom")]
                Initialized { id: SubscriberId, email: Forgettable<String> },
                EmailChanged { email: Forgettable<String> },
            }
        };

        let out = derive(input).unwrap().to_string();

        // Both variants keyed by the snake_case ident, matching `event_type()`.
        assert!(
            out.contains(r#"("initialized" , "email")"#),
            "expected snake_case ident key, got: {out}"
        );
        assert!(
            out.contains(r#"("email_changed" , "email")"#),
            "expected snake_case ident key, got: {out}"
        );
        // Never the serde tag or the rename_all casing.
        assert!(!out.contains(r#""totally_custom""#));
        assert!(!out.contains(r#""emailChanged""#));
    }
}
