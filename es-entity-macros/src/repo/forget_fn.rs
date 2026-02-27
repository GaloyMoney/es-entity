use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct ForgetFn<'a> {
    id: &'a syn::Ident,
    error: &'a syn::Type,
    forgettable_table_name: &'a str,
}

impl<'a> ForgetFn<'a> {
    pub fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            id: opts.id(),
            error: opts.err(),
            forgettable_table_name: opts
                .forgettable_table_name()
                .expect("forgettable must be enabled"),
        }
    }
}

impl ToTokens for ForgetFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let id_type = &self.id;
        let error = self.error;

        let query = format!(
            "DELETE FROM {} WHERE entity_id = $1",
            self.forgettable_table_name
        );

        tokens.append_all(quote! {
            pub async fn forget(
                &self,
                id: impl std::borrow::Borrow<#id_type>
            ) -> Result<(), #error> {
                let mut op = self.begin_op().await?;
                self.forget_in_op(&mut op, id).await?;
                op.commit().await?;
                Ok(())
            }

            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<#id_type>
            ) -> Result<(), #error>
            where
                OP: es_entity::AtomicOperation
            {
                let id = id.borrow();
                sqlx::query!(
                    #query,
                    id as &#id_type
                )
                .execute(op.as_executor())
                .await?;
                Ok(())
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn forget_fn() {
        let id = Ident::new("EntityId", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        let forget_fn = ForgetFn {
            id: &id,
            error: &error,
            forgettable_table_name: "entities_forgettable_payloads",
        };

        let mut tokens = TokenStream::new();
        forget_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn forget(
                &self,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<(), es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                self.forget_in_op(&mut op, id).await?;
                op.commit().await?;
                Ok(())
            }

            pub async fn forget_in_op<OP>(
                &self,
                op: &mut OP,
                id: impl std::borrow::Borrow<EntityId>
            ) -> Result<(), es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let id = id.borrow();
                sqlx::query!(
                    "DELETE FROM entities_forgettable_payloads WHERE entity_id = $1",
                    id as &EntityId
                )
                .execute(op.as_executor())
                .await?;
                Ok(())
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}
