use convert_case::{Case, Casing};
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

/// Information about forgettable fields in an event enum.
struct ForgettableInfo {
    /// Whether any variant has forgettable fields.
    has_forgettable: bool,
    /// Per-variant: (serde_tag_value, list_of_forgettable_field_names)
    variants: Vec<(String, Vec<String>)>,
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
        .map(|(tag_value, field_names)| {
            let field_name_strs: Vec<&str> = field_names.iter().map(|s| s.as_str()).collect();
            quote! {
                Some(#tag_value) => &[#(#field_name_strs),*],
            }
        })
        .collect();

    tokens.append_all(quote! {
        impl #ident {
            #[doc(hidden)]
            pub const HAS_FORGETTABLE_FIELDS: bool = #has_forgettable;

            #[doc(hidden)]
            pub fn forgettable_field_names(
                event_json: &es_entity::prelude::serde_json::Value,
            ) -> &'static [&'static str] {
                match event_json.get("type").and_then(|v| v.as_str()) {
                    #(#match_arms)*
                    _ => &[],
                }
            }

            #[doc(hidden)]
            pub fn extract_forgettable_payload(
                event_json: &mut es_entity::prelude::serde_json::Value,
            ) -> Option<es_entity::prelude::serde_json::Value> {
                let field_names = Self::forgettable_field_names(event_json);
                es_entity::forgettable::extract_forgettable_payload(event_json, field_names)
            }
        }
    });

    Ok(tokens)
}

/// Extract forgettable field information from the enum definition.
fn extract_forgettable_info(ast: &syn::DeriveInput) -> ForgettableInfo {
    let rename_rule = parse_serde_rename_all(ast);

    let variants = match &ast.data {
        syn::Data::Enum(data) => data
            .variants
            .iter()
            .map(|variant| {
                let tag_value = serde_variant_name(variant, &rename_rule);
                let forgettable_fields = variant
                    .fields
                    .iter()
                    .filter_map(|field| {
                        if is_forgettable_type(&field.ty) {
                            field.ident.as_ref().map(|i| i.to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                (tag_value, forgettable_fields)
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

/// Check if a type's last path segment is "Forgettable".
fn is_forgettable_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "Forgettable";
    }
    false
}

/// Parse the `rename_all` value from `#[serde(tag = "type", rename_all = "...")]`.
fn parse_serde_rename_all(ast: &syn::DeriveInput) -> Option<String> {
    for attr in &ast.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut rename_all_str = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                rename_all_str = Some(lit.value());
            } else {
                // Consume any value so parse_nested_meta can continue to the next item
                let _ = meta.value().and_then(|v| v.parse::<syn::LitStr>());
            }
            Ok(())
        });
        if rename_all_str.is_some() {
            return rename_all_str;
        }
    }
    None
}

/// Convert a serde rename_all string to a convert_case::Case.
fn serde_rename_to_case(s: &str) -> Option<Case<'static>> {
    match s {
        "lowercase" => Some(Case::Lower),
        "UPPERCASE" => Some(Case::Upper),
        "PascalCase" => Some(Case::Pascal),
        "camelCase" => Some(Case::Camel),
        "snake_case" => Some(Case::Snake),
        "SCREAMING_SNAKE_CASE" => Some(Case::Constant),
        "kebab-case" => Some(Case::Kebab),
        "SCREAMING-KEBAB-CASE" => Some(Case::Cobol),
        _ => None,
    }
}

/// Get the serde tag name for a variant, considering rename_all and per-variant rename.
fn serde_variant_name(variant: &syn::Variant, rename_rule: &Option<String>) -> String {
    // Check for explicit #[serde(rename = "...")]
    for attr in &variant.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let mut explicit_rename = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                explicit_rename = Some(lit.value());
            }
            Ok(())
        });
        if let Some(name) = explicit_rename {
            return name;
        }
    }

    let ident = variant.ident.to_string();
    if let Some(rule) = rename_rule {
        if let Some(case) = serde_rename_to_case(rule) {
            ident.to_case(case)
        } else {
            ident
        }
    } else {
        ident
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
