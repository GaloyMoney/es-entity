use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::{ClockFieldInfo, RepositoryOptions};

pub struct Begin<'a> {
    clock_field: ClockFieldInfo<'a>,
}

impl<'a> From<&'a RepositoryOptions> for Begin<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            clock_field: opts.clock_field(),
        }
    }
}

impl ToTokens for Begin<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let begin_op_body = match &self.clock_field {
            ClockFieldInfo::None => {
                // No clock field - always use global clock
                quote! {
                    self.begin_op_with_clock(es_entity::clock::Clock::handle()).await
                }
            }
            ClockFieldInfo::Optional(clock_field) => {
                // Optional clock field - use if Some, fallback to global
                quote! {
                    match &self.#clock_field {
                        Some(clock) => self.begin_op_with_clock(clock).await,
                        None => self.begin_op_with_clock(es_entity::clock::Clock::handle()).await,
                    }
                }
            }
            ClockFieldInfo::Required(clock_field) => {
                // Required clock field - always use it
                quote! {
                    self.begin_op_with_clock(&self.#clock_field).await
                }
            }
        };

        tokens.append_all(quote! {
            #[inline(always)]
            pub async fn begin_op(&self) -> Result<es_entity::DbOp<'static>, sqlx::Error> {
                #begin_op_body
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
