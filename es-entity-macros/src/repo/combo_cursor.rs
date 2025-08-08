use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::{list_by_fn::CursorStruct, options::*};

pub struct ComboCursor<'a> {
    entity: &'a syn::Ident,
    cursors: Vec<CursorStruct<'a>>,
}

impl<'a> ComboCursor<'a> {
    pub fn new(opts: &'a RepositoryOptions, cursors: Vec<CursorStruct<'a>>) -> Self {
        Self {
            entity: opts.entity(),
            cursors,
        }
    }

    #[cfg(test)]
    pub fn new_test(entity: &'a syn::Ident, cursors: Vec<CursorStruct<'a>>) -> Self {
        Self { entity, cursors }
    }

    pub fn ident(&self) -> syn::Ident {
        let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
        syn::Ident::new(
            &format!("{entity_name}_cursor").to_case(Case::UpperCamel),
            Span::call_site(),
        )
    }

    pub fn tag(column: &Column) -> syn::Ident {
        let tag_name = format!("By{}", column.name());
        syn::Ident::new(&tag_name, Span::call_site())
    }

    pub fn variants(&self) -> TokenStream {
        let variants = self
            .cursors
            .iter()
            .map(|cursor| {
                let tag = Self::tag(cursor.column);
                let ident = cursor.ident();
                quote! {
                    #tag(#ident),
                }
            })
            .collect::<TokenStream>();

        quote! {
            #variants
        }
    }

    pub fn trait_impls(&self) -> TokenStream {
        let self_ident = self.ident();
        let trait_impls = self
            .cursors
            .iter()
            .map(|cursor| {
                let tag =
                    syn::Ident::new(&format!("By{}", cursor.column.name()), Span::call_site());
                let ident = cursor.ident();
                quote! {
                    impl From<#ident> for #self_ident {
                        fn from(cursor: #ident) -> Self {
                            Self::#tag(cursor)
                        }
                    }

                    impl TryFrom<#self_ident> for #ident {
                        type Error = es_entity::CursorDestructureError;

                        fn try_from(cursor: #self_ident) -> Result<Self, Self::Error> {
                            match cursor {
                                #self_ident::#tag(cursor) => Ok(cursor),
                                _ => Err(es_entity::CursorDestructureError::from((stringify!(#self_ident), stringify!(#ident)))),
                            }
                        }
                    }
                }
            })
            .collect::<TokenStream>();

        quote! {
            #trait_impls
        }
    }

    pub fn sort_by_name(&self) -> syn::Ident {
        let entity_name = pluralizer::pluralize(&format!("{}", self.entity), 2, false);
        syn::Ident::new(
            &format!("{entity_name}_sort_by").to_case(Case::UpperCamel),
            Span::call_site(),
        )
    }

    pub fn sort_by(&self) -> TokenStream {
        let mut default = true;
        let variants = self.cursors.iter().map(|cursor| {
            let name = syn::Ident::new(
                &format!("{}", cursor.column.name()).to_case(Case::UpperCamel),
                Span::call_site(),
            );
            if default {
                default = false;
                quote! {
                    #[default]
                    #name
                }
            } else {
                quote! {
                    #name
                }
            }
        });
        let name = self.sort_by_name();
        #[cfg(feature = "graphql")]
        let mod_name = syn::Ident::new(&format!("{name}").to_case(Case::Snake), Span::call_site());
        #[cfg(feature = "graphql")]
        let sort_by_enum = quote! {
            mod #mod_name {
                use es_entity::graphql::async_graphql;
                #[derive(async_graphql::Enum, Default, Debug, Clone, Copy, PartialEq, Eq)]
                pub enum #name {
                    #(#variants),*
                }
            }
            pub use #mod_name::#name;
        };
        #[cfg(not(feature = "graphql"))]
        let sort_by_enum = quote! {
            #[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
            pub enum #name {
                #(#variants),*
            }
        };
        quote! {
            #sort_by_enum
        }
    }

    #[cfg(feature = "graphql")]
    pub fn gql_cursor(&self) -> TokenStream {
        let ident = self.ident();
        quote! {
            impl es_entity::graphql::async_graphql::connection::CursorType for #ident {
                type Error = String;

                fn encode_cursor(&self) -> String {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let json = es_entity::prelude::serde_json::to_string(&self).expect("could not serialize token");
                    general_purpose::STANDARD_NO_PAD.encode(json.as_bytes())
                }

                fn decode_cursor(s: &str) -> Result<Self, Self::Error> {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let bytes = general_purpose::STANDARD_NO_PAD
                        .decode(s.as_bytes())
                        .map_err(|e| e.to_string())?;
                    let json = String::from_utf8(bytes).map_err(|e| e.to_string())?;
                    es_entity::prelude::serde_json::from_str(&json).map_err(|e| e.to_string())
                }
            }
        }
    }
}

impl ToTokens for ComboCursor<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = self.ident();
        let variants = self.variants();
        let trait_impls = self.trait_impls();

        tokens.append_all(quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            #[allow(clippy::enum_variant_names)]
            #[serde(tag = "type")]
            pub enum #ident {
                #variants
            }

            #trait_impls
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::list_by_fn::CursorStruct;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn combo_cursor_generation() {
        let entity = Ident::new("User", Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());
        let id = syn::Ident::new("UserId", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("UserId").unwrap());
        let name_column = Column::new(
            syn::Ident::new("name", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let name_cursor = CursorStruct {
            column: &name_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let cursors = vec![id_cursor, name_cursor];

        let combo_cursor = ComboCursor {
            entity: &entity,
            cursors,
        };

        let mut tokens = TokenStream::new();
        combo_cursor.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            #[allow(clippy::enum_variant_names)]
            #[serde(tag = "type")]
            pub enum UsersCursor {
                Byid(UsersByIdCursor),
                Byname(UsersByNameCursor),
            }

            impl From<UsersByIdCursor> for UsersCursor {
                fn from(cursor: UsersByIdCursor) -> Self {
                    Self::Byid(cursor)
                }
            }

            impl TryFrom<UsersCursor> for UsersByIdCursor {
                type Error = es_entity::CursorDestructureError;

                fn try_from(cursor: UsersCursor) -> Result<Self, Self::Error> {
                    match cursor {
                        UsersCursor::Byid(cursor) => Ok(cursor),
                        _ => Err(es_entity::CursorDestructureError::from((stringify!(UsersCursor), stringify!(UsersByIdCursor)))),
                    }
                }
            }
            impl From<UsersByNameCursor> for UsersCursor {
                fn from(cursor: UsersByNameCursor) -> Self {
                    Self::Byname(cursor)
                }
            }

            impl TryFrom<UsersCursor> for UsersByNameCursor {
                type Error = es_entity::CursorDestructureError;

                fn try_from(cursor: UsersCursor) -> Result<Self, Self::Error> {
                    match cursor {
                        UsersCursor::Byname(cursor) => Ok(cursor),
                        _ => Err(es_entity::CursorDestructureError::from((stringify!(UsersCursor), stringify!(UsersByNameCursor)))),
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn combo_cursor_sort_by_generation() {
        let entity = Ident::new("Order", Span::call_site());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());
        let id = syn::Ident::new("OrderId", Span::call_site());

        let id_column = Column::for_id(syn::parse_str("OrderId").unwrap());
        let status_column = Column::new(
            syn::Ident::new("status", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );
        let created_at_column = Column::new(
            syn::Ident::new("created_at", proc_macro2::Span::call_site()),
            syn::parse_str("chrono::DateTime<chrono::Utc>").unwrap(),
        );

        let id_cursor = CursorStruct {
            column: &id_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let status_cursor = CursorStruct {
            column: &status_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let created_at_cursor = CursorStruct {
            column: &created_at_column,
            id: &id,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let cursors = vec![id_cursor, status_cursor, created_at_cursor];

        let combo_cursor = ComboCursor {
            entity: &entity,
            cursors,
        };

        let sort_by_tokens = combo_cursor.sort_by();

        let expected = quote! {
            #[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
            pub enum OrdersSortBy {
                #[default]
                Id,
                Status,
                CreatedAt
            }
        };

        assert_eq!(sort_by_tokens.to_string(), expected.to_string());
    }
}
