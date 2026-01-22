use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

pub struct Begin;

impl ToTokens for Begin {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.append_all(quote! {
            #[inline(always)]
            pub async fn begin_op(&self) -> Result<es_entity::DbOp<'static>, sqlx::Error> {
                self.begin_op_with_clock(es_entity::clock::Clock::handle()).await
            }

            #[inline(always)]
            pub async fn begin_op_with_clock(
                &self,
                clock: &es_entity::clock::ClockHandle,
            ) -> Result<es_entity::DbOp<'static>, sqlx::Error> {
                es_entity::DbOp::init_with_clock(self.pool(), clock).await
            }
        });
    }
}
