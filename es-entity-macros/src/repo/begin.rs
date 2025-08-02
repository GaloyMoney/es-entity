use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::RepositoryOptions;

pub struct Begin<'a> {
    op: &'a syn::Type,
    begin: &'a Option<syn::Ident>,
}

impl<'a> From<&'a RepositoryOptions> for Begin<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            op: opts.op(),
            begin: &opts.begin,
        }
    }
}

impl ToTokens for Begin<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let op = &self.op;
        let begin = if let Some(begin) = self.begin {
            quote! {
                self.#begin()
            }
        } else {
            quote! {
                es_entity::DbOp::init(self.pool()).await
            }
        };

        tokens.append_all(quote! {
            #[inline(always)]
            pub async fn begin_op(&self) -> Result<#op, sqlx::Error>{
                #begin
            }
        });
    }
}
