use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::RepositoryOptions;

pub struct Begin<'a> {
    op: &'a syn::Type,
    begin: &'a Option<syn::Ident>,
    is_db_op: bool,
}

impl<'a> From<&'a RepositoryOptions> for Begin<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            op: opts.op(),
            begin: &opts.begin,
            is_db_op: is_db_op_type(opts.op()),
        }
    }
}

/// Check if a type is DbOp (either `DbOp<...>` or `es_entity::DbOp<...>`)
fn is_db_op_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "DbOp";
        }
    }
    false
}

impl ToTokens for Begin<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let op = &self.op;

        if let Some(begin) = self.begin {
            // Custom begin function - just delegate to it
            tokens.append_all(quote! {
                #[inline(always)]
                pub async fn begin_op(&self) -> Result<#op, sqlx::Error> {
                    self.#begin()
                }
            });
        } else if self.is_db_op {
            // DbOp - generate both begin_op and begin_op_with_clock
            tokens.append_all(quote! {
                #[inline(always)]
                pub async fn begin_op(&self) -> Result<#op, sqlx::Error> {
                    self.begin_op_with_clock(es_entity::clock::Clock::handle()).await
                }

                #[inline(always)]
                pub async fn begin_op_with_clock(
                    &self,
                    clock: &es_entity::clock::ClockHandle,
                ) -> Result<#op, sqlx::Error> {
                    es_entity::DbOp::init_with_clock(self.pool(), clock).await
                }
            });
        } else {
            // Non-DbOp type without custom begin - just use DbOp::init
            tokens.append_all(quote! {
                #[inline(always)]
                pub async fn begin_op(&self) -> Result<#op, sqlx::Error> {
                    es_entity::DbOp::init(self.pool()).await
                }
            });
        }
    }
}
